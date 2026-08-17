//! The narrow interface a deterministic, alternating, two-player,
//! zero-sum, perfect-information game must implement to plug into
//! `game-ai-alphabeta`/`game-ai-arena`. See `../DESIGN.md` for the full
//! rationale behind each piece of this trait and what's deliberately
//! left out of it.

use std::hash::Hash;

/// Implementors are zero-sized marker types (e.g. `pub struct
/// OnitamaGame;`) -- every method is a plain associated function, so a
/// generic search function (`fn negamax<G: Game>(...)`) monomorphizes
/// per game with no `dyn Game` and no dynamic dispatch per node.
pub trait Game {
    type State: Copy;
    type Move: Copy + Eq;
    type Player: Copy + Eq;
    /// A key distinguishing positions exactly (no false collisions,
    /// though it need not be a *minimal* encoding). Not required to
    /// equal `State` itself -- see DESIGN.md's note on Onitama's
    /// existing bit-packed key.
    type PositionKey: Copy + Eq + Hash;

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

    /// Static evaluation of `state`, from `current_player(state)`'s
    /// perspective. Higher is better for the player to move. Never
    /// called by a well-behaved search on a state where `result` is
    /// not `InProgress`.
    fn evaluate(state: &Self::State) -> i32;
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
