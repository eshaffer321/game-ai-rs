# game-ai-rs design

A reusable chassis for deterministic, alternating, two-player, zero-sum,
perfect-information games, extracted once two real rules engines
(`onitama-core`, `santorini-core`) existed to shape it against — not
guessed at in advance from one game alone.

## Scope

In scope: deterministic, alternating, two-player, zero-sum,
perfect-information games. That's it.

Explicitly out of scope for this interface: chance (dice, a deal after
the opening), hidden information, more than two players, simultaneous
moves. A future game that needs one of those gets a *new*, additional
capability trait layered alongside this one -- this interface is not
going to grow optional chance/hidden-info hooks nobody implements yet
on the promise that they'll be useful "eventually."

## The `Game` trait

```rust
pub trait Game {
    type State: Copy;
    type Move: Copy + Eq;
    type Player: Copy + Eq;
    type PositionKey: Copy + Eq;

    fn current_player(state: &Self::State) -> Self::Player;
    fn other_player(player: Self::Player) -> Self::Player;
    fn legal_moves(state: &Self::State) -> Vec<Self::Move>;
    fn apply_move(state: &Self::State, mv: Self::Move) -> Self::State;
    fn result(state: &Self::State) -> GameResult<Self::Player>;
    fn position_key(state: &Self::State) -> Self::PositionKey;
    fn tt_hash(key: &Self::PositionKey) -> u64;
}

pub enum GameResult<Player> {
    InProgress,
    Win(Player),
    Draw,
}
```

Implementors are zero-sized marker types (`pub struct OnitamaGame;`,
`pub struct SantoriniGame;`), not structs wrapping game state -- every
method is a plain associated function, mirroring how `onitama-core` and
`santorini-core` already expose free functions over a `GameState`
rather than methods with a `&self` receiver. This is also what makes
`fn negamax<G: Game>(...)` in the search crate monomorphize per game
with no `dyn Game` and no dynamic dispatch per node, per the
constraint that the search loop stay fully generic/static.

Why each piece is shaped the way it is:

- **Associated types, not generic parameters on every function.** Each
  implementor fixes one concrete `(State, Move, Player, PositionKey)`
  tuple; call sites only ever name the marker type.
- **`PositionKey` is its own associated type, distinct from `State`.**
  Onitama already has a bit-packed, collision-free `u128` key (sorted
  hands, etc.) that is *not* just `GameState` itself -- reusing that
  exact scheme is one of the reasons for this extraction, not
  something to redesign. Santorini can start with something as simple
  as its own `State` (it's already small and `Copy`) without the two
  games being forced to share a representation.
- **`other_player` is a trait method, not a bound on `Player`.** Both
  games already have this exact operation (`Color::other`,
  `Player::other`) on their own player enums; requiring `Self::Player:
  SomeOtherPlayerTrait` instead would mean bolting a foreign trait onto
  a type the game crate owns, just to satisfy this interface.
- **No `evaluate` on this trait at all** (an earlier draft had one; see
  "Revision history" below for why it was removed). Evaluation is not
  a rules-engine concern, and a static associated function could never
  carry Onifish's runtime-configurable `EvalWeights` anyway. It lives
  on `game_ai_alphabeta::SearchHooks` instead -- see that section.
- **`tt_hash` is a separate method from `PositionKey`'s equality**, not
  a `Hash` bound on the associated type. A transposition table needs a
  *deterministic*, run-to-run-stable index -- `std`'s `Hash`/`Hasher`
  is explicitly the wrong tool (its default `RandomState` reseeds every
  process), and using it anyway would silently change which positions
  collide in a bounded table, changing node counts and PVs on every
  run even though the search itself hasn't changed. `tt_hash` only
  picks a slot; the full `PositionKey` is still stored per entry and
  compared exactly on probe, so a hash collision only costs a
  wasted/overwritten slot, never a wrong search result. Onitama's
  implementation must reproduce Onifish's existing fmix64-based mixing
  bit for bit, or the frozen-position replay gate (see Migration plan)
  can fail on node count/PV despite logically identical search.
- **`GameResult` keeps a `Draw` variant even though neither game
  produces one today.** Cheap to keep now, expensive to bolt on later
  (every match on `GameResult` across two search implementations would
  need revisiting). A third deterministic game with a repetition/
  move-count draw rule is exactly the kind of thing this interface
  should already be shaped for.

## `game_ai_alphabeta::SearchHooks`

```rust
pub trait SearchHooks<G: Game> {
    fn evaluate(&self, state: &G::State) -> i32;
    fn is_noisy(&self, state: &G::State, mv: &G::Move) -> bool;
    fn has_immediate_threat(&self, state: &G::State, player: G::Player) -> bool;
    fn history_index(&self, mv: &G::Move) -> (usize, usize);
}
```

Everything a search needs from a game that isn't pure rules, kept out
of `Game` and moved here instead:

- **Evaluation.** Onifish's evaluator is not stateless -- it carries
  `EvalWeights`, tunable at runtime (Texel tuning, ablations, the
  benchmark ladder all vary it per player instance). A `Game::evaluate
  (state) -> i32` associated function has nowhere to keep that; a
  `SearchHooks` implementor is an ordinary struct (`OnitamaHooks {
  weights: EvalWeights }`), built once per player and holding whatever
  state its evaluator needs.
- **Noisy/capture/immediate-win classification**, for quiescence.
- **Immediate-threat detection**, for gating late-move reductions.
- **History-table indexing**, since a quiet move's `(from, to)`-style
  bucket is move-representation-specific (Onitama: literal board
  squares; a different game's moves might not decompose that way at
  all).

`SearchHooks` implementors are still ordinary generic type parameters,
not `dyn` objects: `fn negamax<G: Game, H: SearchHooks<G>>(hooks: &H,
...)` monomorphizes per `(G, H)` pair, so a hooks struct carrying
per-player configuration costs nothing extra per node versus the
zero-sized `Game` marker types.

## What's deliberately NOT in either trait

- No `search`, `mcts`, or neural-evaluator methods anywhere. These
  interfaces describe what a search algorithm needs from a *rules
  engine* and from a *technique-agnostic evaluator/classifier*, not
  what any one search technique or model architecture needs.
- No notion of captures, temples, workers, buildings, or cards
  anywhere in `game-ai-core`, and no game-specific tactical rule
  hardcoded into `game-ai-alphabeta` itself -- only the `SearchHooks`
  trait shape. Each game's own classification (Onitama: capture or
  immediate temple win is noisy; Santorini's is presumably a climb
  onto height 3, once that's designed) lives in that game's adapter,
  behind the trait.
- Setup-phase placement gets no special treatment anywhere in either
  trait. Santorini's `Phase::Setup` is just a state whose
  `legal_moves` happen to be `Move::Place` -- neither trait, nor the
  search that runs on top of them, ever needs to know a "setup phase"
  concept exists at all.

## Revision history

The first draft of `Game` included a static `evaluate` method and
required `PositionKey: Hash`. Both were removed before any real search
code was migrated onto the interface, once review against Onifish's
actual engine surfaced why they don't fit:

- `evaluate` couldn't preserve `EvalWeights` being a per-player runtime
  value, and coupled game rules to exactly one evaluator when a game
  might reasonably want several (a classical evaluator and a future
  neural one, say). Replaced by `SearchHooks::evaluate`, above.
- `PositionKey: Hash` handed TT indexing to `std`'s default hasher,
  which reseeds every process -- not the deterministic, reproducible
  mixing Onifish's existing TT already depends on for exact node-count
  and PV parity. Replaced by an explicit `Game::tt_hash`, above.

## Repo layout

```text
game-ai-rs
├─ game-ai-core       the Game trait above; zero dependencies on any
│                      specific game
├─ game-ai-alphabeta   iterative-deepening negamax generic over
│                      G: Game -- TT, quiescence, move ordering,
│                      budgets, diagnostics (not yet migrated; see
│                      Migration plan)
└─ game-ai-arena       game-agnostic self-play/match tooling generic
                       over G: Game (not yet built)
```

Game-specific adapters -- a marker type with its `Game` impl, plus a
`SearchHooks` implementor carrying the game's own evaluator and
tactical/"noisy move" classification -- live in each game's **own**
repository (`onitama`, `santorini-rs`),
depending on `game-ai-core`/`game-ai-alphabeta` via a **pinned Git
revision**, never the reverse. No committed `path = "../..."`
dependency ever crosses a repo boundary in the final arrangement: all
three repositories must stay independently cloneable and buildable on
their own.

## Migration plan

Staged, per the acceptance gates that govern this extraction:

1. **Freeze** a representative set of real Onifish positions (fixed
   depth: move, score, node count, PV) using the current,
   pre-extraction engine, before touching any search code.
2. Land the interface plus a minimal adapter spike for both games --
   just `Game` impls proving the trait shape survives contact with two
   genuinely different rules engines (Onitama's card-based movement
   and captures vs. Santorini's setup phase, king-move adjacency, and
   building), no search migrated yet.
2a. **This commit**: revise the interface itself after that spike
   surfaced two rules-vs-search leaks (evaluation needing to carry
   `EvalWeights`; TT indexing needing to be deterministic, not
   `std::hash`-based) -- see "Revision history" above. Both adapters'
   `Game` impls updated (`tt_hash` added, reproducing Onifish's exact
   mixing for Onitama), and both now also implement `SearchHooks` via
   a small per-game hooks struct.
3. Only once (2a) compiles cleanly for both games: migrate Onifish's
   actual alpha-beta/TT/quiescence/move-ordering/diagnostics into
   `game-ai-alphabeta`, generic over `G: Game, H: SearchHooks<G>`,
   consuming the two adapters' existing `SearchHooks` implementors
   directly rather than reaching into `onitama-ai` for
   `is_capture`/`is_immediate_win`/`EvalWeights` by hand.
4. Re-run the positions frozen in (1) against the migrated engine at
   the same fixed depths -- move, score, node count, and PV must match
   exactly.
5. Benchmark the migrated engine's throughput against the frozen
   (pre-migration) engine; must land within roughly 5% at equal depth.
6. If anything in the actual hot path changed shape during migration
   (not just moved files), run a small equal-time arena control before
   trusting the throughput comparison alone.
7. Give Santorini a simple handcrafted evaluator, plug it into the
   migrated `game-ai-alphabeta`, and confirm alpha-beta completes
   legal games end to end (extending `santorini-core`'s existing
   randomized-complete-game testing to run through the shared search
   instead of random moves).

Steps 3-7 are the next phase of work -- not part of this commit, which
only covers steps 1-2a.
