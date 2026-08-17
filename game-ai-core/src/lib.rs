//! The narrow interface a deterministic, alternating, two-player,
//! zero-sum, perfect-information game must implement to plug into
//! `game-ai-alphabeta`/`game-ai-arena`. See `../DESIGN.md` for the full
//! rationale behind each piece of this trait and what's deliberately
//! left out of it.

/// Implementors are zero-sized marker types (e.g. `pub struct
/// OnitamaGame;`) -- every method is a plain associated function, so a
/// generic search function (`fn negamax<G: Game>(...)`) monomorphizes
/// per game with no `dyn Game` and no dynamic dispatch per node.
///
/// Deliberately *not* here: evaluation, or any tactical classification
/// (captures, "noisy" moves, immediate threats) a search technique
/// might need. A rules engine is not an evaluator -- Onifish's
/// evaluator carries runtime-configurable `EvalWeights` that a static
/// associated function on this trait could never preserve, and a
/// different game may want a completely different evaluator without
/// touching its rules at all. Those live in `game_ai_alphabeta::
/// SearchHooks`, a separate, per-game, monomorphized object the
/// search actually runs against -- see that trait's doc comment and
/// DESIGN.md.
pub trait Game {
    type State: Copy;
    type Move: Copy + Eq;
    type Player: Copy + Eq;
    /// A key distinguishing positions exactly (no false collisions,
    /// though it need not be a *minimal* encoding). Not required to
    /// equal `State` itself -- see DESIGN.md's note on Onitama's
    /// existing bit-packed key.
    type PositionKey: Copy + Eq;

    /// Whose turn it is to move in `state`.
    fn current_player(state: &Self::State) -> Self::Player;

    /// The other player, independent of any particular state.
    fn other_player(player: Self::Player) -> Self::Player;

    /// Every legal move for `current_player(state)`. Empty only when
    /// the game has already ended (see `result`), or when the side to
    /// move is stalemated -- itself a loss, distinguished by `result`.
    fn legal_moves(state: &Self::State) -> Vec<Self::Move>;

    /// Applies `mv` (assumed legal -- callers should only ever pass
    /// moves produced by `legal_moves`) and returns the resulting
    /// state.
    fn apply_move(state: &Self::State, mv: Self::Move) -> Self::State;

    /// Whether the game is over, and who (if anyone) won.
    fn result(state: &Self::State) -> GameResult<Self::Player>;

    /// An exact key for `state`: two states with the same key must be
    /// equivalent for every purpose a search cares about (legal moves,
    /// result, evaluation), and two inequivalent states must never
    /// share a key.
    fn position_key(state: &Self::State) -> Self::PositionKey;

    /// A **deterministic** hash of `key`, used only to pick a slot in
    /// a bounded transposition table -- never to distinguish
    /// positions on its own (a TT entry still stores the full
    /// `PositionKey` and compares it exactly on probe, so a `tt_hash`
    /// collision only costs a wasted or overwritten slot, never a
    /// wrong result). Must be stable run to run: `std`'s default
    /// `Hash`/`Hasher` is explicitly unsuitable here (its
    /// `RandomState` seed varies per process), which is why this is a
    /// dedicated method rather than a `Hash` bound on `PositionKey`.
    /// Onitama's implementation must reproduce Onifish's exact
    /// existing mixing (a folded fmix64 finalizer) bit for bit --
    /// changing it changes which positions collide in the table,
    /// which changes node counts and PVs even though the search
    /// itself is unchanged, breaking the frozen-position replay gate.
    fn tt_hash(key: &Self::PositionKey) -> u64;
}

/// The outcome of a game at a given state. `Draw` has no producer in
/// either game implemented so far, but is kept as a first-class
/// variant deliberately -- see DESIGN.md.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GameResult<Player> {
    InProgress,
    Win(Player),
    Draw,
}

impl<Player> GameResult<Player> {
    pub fn is_over(&self) -> bool {
        !matches!(self, GameResult::InProgress)
    }
}

#[cfg(test)]
mod tests;
