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
    type PositionKey: Copy + Eq + Hash;

    fn current_player(state: &Self::State) -> Self::Player;
    fn other_player(player: Self::Player) -> Self::Player;
    fn legal_moves(state: &Self::State) -> Vec<Self::Move>;
    fn apply_move(state: &Self::State, mv: Self::Move) -> Self::State;
    fn result(state: &Self::State) -> GameResult<Self::Player>;
    fn position_key(state: &Self::State) -> Self::PositionKey;
    fn evaluate(state: &Self::State) -> i32;
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
- **`evaluate` returns a plain `i32`, from `state`'s current player's
  perspective** (matching Onitama's existing convention exactly). This
  is deliberately the *only* evaluation hook in the trait -- a single
  static/handcrafted evaluator. Anything else (a neural policy/value
  head, MCTS) is out of scope for this interface; see below.
- **`GameResult` keeps a `Draw` variant even though neither game
  produces one today.** Cheap to keep now, expensive to bolt on later
  (every match on `GameResult` across two search implementations would
  need revisiting). A third deterministic game with a repetition/
  move-count draw rule is exactly the kind of thing this interface
  should already be shaped for.

## What's deliberately NOT in the trait

- No `search`, `mcts`, or neural-evaluator methods. This interface
  describes what a search algorithm needs from a *rules engine*, not
  what any one search technique needs from a game.
- No notion of captures, temples, workers, buildings, or cards
  anywhere in `game-ai-core`. Quiescence's "is this move tactically
  noisy" classification is necessarily game-specific (Onitama: capture
  or immediate temple win; Santorini: presumably a climb onto height 3,
  once that's designed) and belongs behind a *separate*, per-game
  extension point in `game-ai-alphabeta` -- not hardcoded against
  Onitama's specific rules the way the original engine was.
- Setup-phase placement gets no special treatment anywhere in this
  trait. Santorini's `Phase::Setup` is just a state whose
  `legal_moves` happen to be `Move::Place` -- the trait, and the
  search that runs on top of it, never need to know a "setup phase"
  concept exists at all.

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

Game-specific adapters -- a marker type, its `Game` impl, the game's
own evaluator, and its own tactical/"noisy move" classification -- live
in each game's **own** repository (`onitama`, `santorini-rs`),
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
2. **This commit**: land the interface above plus a minimal adapter
   spike for both games -- just `Game` impls proving the trait shape
   survives contact with two genuinely different rules engines
   (Onitama's card-based movement and captures vs. Santorini's setup
   phase, king-move adjacency, and building), no search migrated yet.
3. Only once (2) compiles cleanly for both games: migrate Onifish's
   actual alpha-beta/TT/quiescence/move-ordering/diagnostics into
   `game-ai-alphabeta`, generic over `G: Game`, with Onitama's
   tactical classification (`is_capture`/`is_immediate_win`) and
   `EvalWeights` moved into an Onitama-side adapter that plugs into
   the generic engine's per-game extension points.
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
only covers steps 1-2.
