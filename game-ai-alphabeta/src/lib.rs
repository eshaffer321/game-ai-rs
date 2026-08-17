//! Iterative-deepening negamax generic over `G: game_ai_core::Game` --
//! the search algorithm itself is not yet migrated. See
//! `../DESIGN.md`'s Migration plan: this crate is intentionally just
//! the `SearchHooks` interface until the interface spike (both game
//! adapters compiling against `game-ai-core` and `SearchHooks`)
//! survives contact with two real, differently-shaped rules engines.

pub use game_ai_core::Game;

/// Everything a search needs from a game that isn't pure rules --
/// deliberately kept out of `Game` itself (see that trait's doc
/// comment) because it's evaluator/technique-specific, not
/// rules-specific: a game's evaluator can carry its own runtime state
/// (Onifish's `EvalWeights`), and different search techniques might
/// want different tactical classifications entirely.
///
/// Implementors are ordinary (not zero-sized) structs -- e.g. an
/// Onitama hooks type owning `EvalWeights` -- built once per player/
/// search configuration, not per node. A generic search function
/// (`fn negamax<G: Game, H: SearchHooks<G>>(hooks: &H, ...)`)
/// monomorphizes per `(G, H)` pair, so there's still no dynamic
/// dispatch per node -- `&dyn SearchHooks<G>` is never used in the
/// hot path.
pub trait SearchHooks<G: Game> {
    /// Static evaluation of `state`, from `G::current_player(state)`'s
    /// perspective. Higher is better for the player to move. Never
    /// called by a well-behaved search on a state where `G::result`
    /// is not `InProgress`.
    fn evaluate(&self, state: &G::State) -> i32;

    /// Whether `mv`, played at `state`, is tactically "noisy" -- worth
    /// extending through in quiescence search rather than cut off at
    /// the normal horizon. Game-specific: Onitama's is a capture or an
    /// immediate temple win; a different game may define this
    /// completely differently (or not use quiescence at all).
    fn is_noisy(&self, state: &G::State, mv: &G::Move) -> bool;

    /// Whether `player` currently has some move at `state` that would
    /// win outright or otherwise represents a decisive tactical
    /// threat. Used to gate late-move reductions: a search never
    /// reduces *any* move while this holds for either player, since a
    /// sharp position is exactly where a shallow trial search is most
    /// likely to misjudge a quiet move's value.
    fn has_immediate_threat(&self, state: &G::State, player: G::Player) -> bool;

    /// Maps a quiet move to its slot in the search's history table, as
    /// `(from_index, to_index)`. Onitama's is the move's board
    /// `(from, to)` squares; a game whose moves don't decompose that
    /// way can still pick any two indices that usefully bucket its
    /// quiet moves (e.g. hashing the move into a fixed range).
    fn history_index(&self, mv: &G::Move) -> (usize, usize);
}

#[cfg(test)]
mod tests;
