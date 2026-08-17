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
    /// Size of the flat history table this implementor's
    /// `MoveFeatures::history_bucket`s index into. Onitama: `25 * 25 =
    /// 625`, one bucket per `(from, to)` board-square pair. A single
    /// declared size (not a `(usize, usize)` pair, as an earlier draft
    /// of this trait had) so a game whose moves don't decompose into
    /// two independent indices isn't forced to invent a fake second
    /// dimension just to satisfy the shape.
    const HISTORY_BUCKETS: usize;

    /// Static evaluation of `state`, from `G::current_player(state)`'s
    /// perspective. Higher is better for the player to move. Never
    /// called by a well-behaved search on a state where `G::result`
    /// is not `InProgress`.
    fn evaluate(&self, state: &G::State) -> i32;

    /// Everything move ordering, quiescence, and the history table
    /// need to know about `mv`, played at `state`, computed together
    /// so all three can never disagree with each other about what kind
    /// of move this is (an earlier draft split this into two methods,
    /// `is_noisy` and `history_index`, which is exactly what let a
    /// `Move::Pass`-shaped gap slip through: nothing forced the noisy
    /// classification and the history bucket to agree that a pass has
    /// neither). See `MoveFeatures`.
    fn move_features(&self, state: &G::State, mv: &G::Move) -> MoveFeatures;

    /// Whether `player` currently has some move at `state` that would
    /// win outright or otherwise represents a decisive tactical
    /// threat. Used to gate late-move reductions: a search never
    /// reduces *any* move while this holds for either player, since a
    /// sharp position is exactly where a shallow trial search is most
    /// likely to misjudge a quiet move's value.
    fn has_immediate_threat(&self, state: &G::State, player: G::Player) -> bool;
}

/// A move's role in ordering, quiescence, and history -- one call's
/// worth of everything `SearchHooks` needs to know about a single
/// move, rather than several methods that each have to independently
/// agree on the same classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveFeatures {
    /// Coarse move-ordering tier: immediate wins first, then captures,
    /// then everything else, then a pass (if the game has one) last.
    /// Finer-grained ordering within a tier (TT move, killers, history
    /// score) is the search's job, not this trait's.
    pub priority: MovePriority,
    /// Whether this move is tactically "noisy" -- worth extending
    /// through in quiescence search rather than cut off at the normal
    /// horizon. Not always identical to `priority == Capture`: Onitama
    /// treats an immediate win as noisy too (`priority ==
    /// ImmediateWin`), regardless of whether it happens to also be a
    /// capture.
    pub is_noisy: bool,
    /// Whether this move captures something. Kept distinct from
    /// `priority` because a priority tier alone can't recover it: an
    /// `ImmediateWin` might or might not also be a capture (Onitama:
    /// capturing the opponent's master is an immediate win *and* a
    /// capture; walking the master onto the opponent's temple is an
    /// immediate win but *not* a capture), and callers that only care
    /// about "did this take something" need the distinction on its own
    /// -- notably, `history_bucket` gating (see below).
    pub is_capture: bool,
    /// This move's slot in the flat, `HISTORY_BUCKETS`-sized history
    /// table, or `None` if it should never be recorded there at all.
    /// `None` for anything that isn't a genuinely quiet move worth
    /// remembering as "usually good" (Onitama: any capture, matching
    /// the existing engine's `if !is_capture { record }` gate exactly
    /// -- including the edge case of a non-capturing immediate win,
    /// which the existing engine's gate does *not* exclude, so this
    /// mirrors that rather than "cleaning it up"), and always `None`
    /// for a pass or anything else with no real board move to bucket.
    pub history_bucket: Option<usize>,
}

/// See `MoveFeatures::priority`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovePriority {
    ImmediateWin,
    Capture,
    Ordinary,
    /// A move that doesn't actually change the board -- Onitama's
    /// `Move::Pass` (still exchanges a card, but no piece moves).
    /// Always ordered last; never noisy, never a capture, never has a
    /// history bucket. Games with no such move simply never produce
    /// this variant.
    Pass,
}

#[cfg(test)]
mod tests;
