//! Iterative-deepening negamax generic over `G: Game, H: SearchHooks<G>`
//! -- a mechanical port of Onifish's alpha-beta engine
//! (`onitama-ai::alphabeta`), monomorphized per `(G, H)` pair instead
//! of hardcoded against `onitama_core::GameState`. Ported to preserve
//! behavior exactly, not to redesign: every technique, default, and
//! quirk here (including ones that read as inconsistencies, e.g. a
//! non-capturing immediate win still getting a history-table entry)
//! matches the original on purpose -- see the frozen-position replay
//! gate this crate's consumers run against.

use web_time::{Duration, Instant};

use game_ai_core::{Game, GameResult};

use crate::{MovePriority, SearchHooks};

/// Score just below the largest terminal value ever produced
/// (`MATE - ply`), so an alpha-beta window of `[-MATE-1, MATE+1]`
/// comfortably brackets every possible score without overflowing.
pub const MATE: i32 = 30_000;

// ---------------------------------------------------------------------
// Transposition table
// ---------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Bound {
    Exact,
    Lower,
    Upper,
}

struct TtEntry<G: Game> {
    key: G::PositionKey,
    depth: u8,
    // `bound`/`score` are written on every store but no longer read by
    // `negamax` — TT entries are advisory-only (move-ordering hint via
    // `best_move` only) since a cached score can't be trusted across
    // different ancestor paths (the Graph History Interaction problem;
    // see negamax's TT-probe comment). Kept, not deleted, because they're
    // exactly the data a future path-aware authoritative TT would need;
    // stripping them now would just mean re-adding them later.
    #[allow(dead_code)]
    bound: Bound,
    #[allow(dead_code)]
    score: i32,
    best_move: Option<G::Move>,
    generation: u32,
}

// Manual impls: `#[derive(Clone, Copy)]` would incorrectly require
// `G: Clone + Copy` itself (a zero-sized marker type has no reason to
// be either), when only `G::PositionKey`/`G::Move` (already `Copy`
// per the `Game` trait's bounds) need to be.
impl<G: Game> Clone for TtEntry<G> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<G: Game> Copy for TtEntry<G> {}

struct TranspositionTable<G: Game> {
    slots: Vec<Option<TtEntry<G>>>,
    capacity: usize,
    generation: u32,
}

impl<G: Game> TranspositionTable<G> {
    fn new(megabytes: usize) -> Self {
        let entry_size = std::mem::size_of::<Option<TtEntry<G>>>().max(1);
        let capacity = ((megabytes.max(1) * 1024 * 1024) / entry_size).max(1);
        TranspositionTable { slots: vec![None; capacity], capacity, generation: 0 }
    }

    fn index(&self, key: G::PositionKey) -> usize {
        (G::tt_hash(&key) % self.capacity as u64) as usize
    }

    fn probe(&self, key: G::PositionKey) -> Option<TtEntry<G>> {
        self.slots[self.index(key)].filter(|e| e.key == key)
    }

    /// Replaces the slot unless it already holds a same-or-newer-
    /// generation entry from a *deeper* search — a shallower result is
    /// never more informative than what's already there.
    fn store(&mut self, key: G::PositionKey, depth: u8, bound: Bound, score: i32, best_move: Option<G::Move>) {
        let idx = self.index(key);
        let replace = match &self.slots[idx] {
            None => true,
            Some(existing) => existing.generation != self.generation || existing.depth <= depth,
        };
        if replace {
            self.slots[idx] = Some(TtEntry { key, depth, bound, score, best_move, generation: self.generation });
        }
    }

    fn new_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}

// ---------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------

/// What bounds an `analyze` call. Node and depth limits are the primary
/// deterministic test/benchmark controls (byte-for-byte reproducible);
/// `MoveTime` checks its deadline inside recursion and returns whatever
/// the last *fully completed* iterative-deepening depth found — a
/// timeout never returns a partially searched depth's result.
#[derive(Clone, Copy, Debug)]
pub enum SearchLimit {
    Depth(u8),
    Nodes(u64),
    MoveTime(Duration),
}

#[derive(Clone, Debug)]
pub struct AlphaBetaConfig {
    pub limit: SearchLimit,
    pub tt_megabytes: usize,
    pub max_ply: usize,
    /// Caps how many extra plies quiescence search may extend beyond the
    /// normal iterative-deepening horizon, searching only "noisy" moves
    /// (`MoveFeatures::is_noisy`) to avoid misjudging a position that's
    /// mid-exchange right at the horizon. `None` disables quiescence
    /// entirely — the horizon simply returns a static evaluation.
    pub quiescence_max_extra_ply: Option<u8>,
    /// Enables principal variation search: after the first move at a
    /// node (assumed best by move ordering), later siblings are first
    /// probed with a cheap null (zero-width) window to test whether they
    /// can beat alpha at all, and only re-searched with the full window
    /// if that scout suggests they might actually be better. `false`
    /// searches every move at the normal full window. A pure
    /// search-efficiency technique: with correct alpha-beta bounds, it
    /// must never change which move or score a completed fixed-depth
    /// search returns, only how many nodes it costs to get there.
    pub pvs: bool,
    /// Half-width of the root's aspiration window: from iterative
    /// deepening's second iteration onward, each depth is first searched
    /// with a narrow window centered on the *previous* completed depth's
    /// score instead of the full range. A failed narrow search is
    /// simply re-searched with the full window, which is enough to
    /// guarantee the same exact score a full-window search would have
    /// returned. `None` disables aspiration windows.
    pub aspiration_window: Option<i32>,
    /// Enables killer-move ordering: a quiet move that caused a beta
    /// cutoff gets tried early in sibling branches at the same ply.
    /// Independent of `history_heuristic`. Move-ordering only; disabling
    /// it cannot change a completed search's score, only how many nodes
    /// it costs.
    pub killer_moves: bool,
    /// Enables the history heuristic: a quiet move that caused a beta
    /// cutoff accumulates a bonus at its `MoveFeatures::history_bucket`
    /// entry, tried early elsewhere in the tree even at different plies.
    /// Independent of `killer_moves`. Move-ordering only.
    pub history_heuristic: bool,
    /// How much a quiet move's history-table entry grows on a beta
    /// cutoff, as a function of the remaining search `depth` at which
    /// the cutoff occurred. Only meaningful when `history_heuristic` is
    /// `true`.
    pub history_bonus: HistoryBonus,
    /// Enables late-move reductions: a conservative subset of quiet
    /// moves gets searched one ply shallower first, only re-searched at
    /// full depth if that shallower search suggests it might actually
    /// beat alpha. See `is_lmr_eligible` for the exact conditions.
    /// Unlike quiescence/PVS/aspiration windows, this is *not* provably
    /// score-preserving at a fixed depth. `false` (the default) disables
    /// it entirely.
    pub lmr: bool,
    /// Benchmarking-only toggle: `true` (the default) uses
    /// `sort_by_cached_key` in `order_moves`; `false` uses
    /// `sort_by_key`, which re-invokes the scoring closure on every
    /// comparison the sort performs rather than once per move. Both
    /// produce byte-identical output -- this exists solely so callers
    /// can A/B the two for an equal-time arena screen.
    pub order_moves_use_cached_key: bool,
}

/// See `AlphaBetaConfig::history_bonus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryBonus {
    /// `depth²` — the engine's original formula.
    DepthSquared,
    /// `depth` — linear in remaining depth.
    Depth,
    /// `1` — Schaeffer's original history heuristic: a plain cutoff
    /// counter, independent of depth entirely.
    Flat,
    /// `min(depth², cap)` — `DepthSquared`, clamped.
    DepthSquaredCapped(i32),
}

impl HistoryBonus {
    fn value(self, depth: u8) -> i32 {
        let depth = depth as i32;
        match self {
            HistoryBonus::DepthSquared => depth * depth,
            HistoryBonus::Depth => depth,
            HistoryBonus::Flat => 1,
            HistoryBonus::DepthSquaredCapped(cap) => (depth * depth).min(cap),
        }
    }
}

impl Default for AlphaBetaConfig {
    fn default() -> Self {
        AlphaBetaConfig {
            limit: SearchLimit::Depth(4),
            tt_megabytes: 32,
            max_ply: 64,
            quiescence_max_extra_ply: Some(4),
            pvs: false,
            aspiration_window: None,
            killer_moves: true,
            history_heuristic: true,
            history_bonus: HistoryBonus::DepthSquared,
            lmr: false,
            order_moves_use_cached_key: true,
        }
    }
}

/// Minimum remaining `depth` (before reduction) for a move to be
/// LMR-eligible at all — "meaningful remaining depth."
const LMR_MIN_DEPTH: u8 = 4;
/// Minimum 0-based move index before LMR considers a move "late" — the
/// first few moves (in TT-move/immediate-win/capture/killer/history
/// order) are never reduced regardless of their own type, since even a
/// quiet move that early in the ordering might still be the best move.
const LMR_MIN_MOVE_INDEX: usize = 3;
/// Ply reduction applied to an eligible move's trial search.
const LMR_REDUCTION: u8 = 1;

/// A neural (or otherwise learned) root-only move-ordering hint --
/// deliberately narrow: a single call's worth of `(move, probability)`
/// pairs plus a value estimate, consulted only at the search root (see
/// `order_root_moves`). Kept separate from `SearchHooks::evaluate`
/// (the classical evaluator remains the only source of interior-node
/// scores) the same way Onifish's original `root_policy_evaluator`
/// field was separate from `EvalWeights`.
pub trait RootPolicyEvaluator<G: Game> {
    fn evaluate(&self, state: &G::State) -> (Vec<(G::Move, f32)>, f32);
}

#[derive(Clone, Debug)]
pub struct AlphaBetaAnalysis<G: Game> {
    pub best_move: G::Move,
    pub score: i32,
    pub completed_depth: u8,
    pub selective_depth: u8,
    pub nodes: u64,
    /// Subset of `nodes` spent in quiescence search specifically (0 if
    /// disabled) — reported separately since it's a materially different
    /// kind of work (noisy-moves-only, not a full move-loop node).
    pub quiescence_nodes: u64,
    pub tt_hits: u64,
    pub beta_cutoffs: u64,
    /// Number of times a PVS null-window scout suggested a move might
    /// beat alpha and had to be re-searched at the full window (0 if
    /// PVS is disabled).
    pub pvs_researches: u64,
    /// Number of iterative-deepening depths (from depth 2 onward) whose
    /// aspiration-window search failed and had to be re-searched at the
    /// full window (0 if aspiration windows are disabled).
    pub aspiration_researches: u64,
    /// Number of moves searched at a reduced depth under LMR (0 if
    /// disabled).
    pub lmr_reductions: u64,
    /// Of `lmr_reductions`, how many suggested an alpha improvement and
    /// were re-searched at full depth.
    pub lmr_researches: u64,
    /// How many times the root-policy evaluator was actually called
    /// this move (0 if none was passed to `analyze`, or the position
    /// had only one legal move; otherwise always exactly 1, however
    /// many iterative-deepening depths were searched — the policy is
    /// cached after its one call).
    pub root_policy_calls: u64,
    /// Whether the (one, if any) policy call actually got used for root
    /// ordering. `false` whenever `root_policy_calls == 0`, and also
    /// `false` if a call happened but its output failed validation
    /// (NaN/Inf values, or a move set mismatch) — in which case the
    /// root fell back to fully classical ordering.
    pub root_policy_used: bool,
    /// Wall-clock time actually spent inside the one root-policy
    /// evaluator call this move, if any.
    pub root_policy_time: Duration,
    pub elapsed: Duration,
    pub principal_variation: Vec<G::Move>,
}

struct SearchContext<G: Game> {
    tt: TranspositionTable<G>,
    killers: Vec<[Option<G::Move>; 2]>,
    history: Vec<i32>,
    nodes: u64,
    quiescence_nodes: u64,
    tt_hits: u64,
    beta_cutoffs: u64,
    pvs_researches: u64,
    lmr_reductions: u64,
    lmr_researches: u64,
    quiescence_max_extra_ply: Option<u8>,
    pvs: bool,
    killer_moves: bool,
    history_heuristic: bool,
    history_bonus: HistoryBonus,
    lmr: bool,
    order_moves_use_cached_key: bool,
    /// Cached once per `analyze()` call (never re-requested across
    /// iterative-deepening depths) and consulted only at the root
    /// (`ply == 0`) — see `order_root_moves`. `None` means either no
    /// evaluator was passed or its output failed validation, in which
    /// case the root falls back to entirely classical ordering,
    /// byte-identical to `order_moves`.
    root_policy: Option<Vec<(G::Move, f32)>>,
    node_limit: Option<u64>,
    deadline: Option<Instant>,
    aborted: bool,
    /// Position keys of every ancestor on the current search line (not
    /// including the node currently being searched) — used to detect an
    /// exact repetition within this line, scored as a draw. Ancestors
    /// only, not the whole game's history.
    path: Vec<G::PositionKey>,
    max_ply: usize,
    /// `config.tt_megabytes > 0`. Kept as an explicit bypass (rather than
    /// just relying on a tiny table) so "TT off" is a real, testable
    /// mode: the transposition table is a pure speed optimization and
    /// must never change which move or score a search returns.
    use_tt: bool,
}

fn check_limits<G: Game>(ctx: &mut SearchContext<G>) {
    if let Some(limit) = ctx.node_limit {
        if ctx.nodes >= limit {
            ctx.aborted = true;
            return;
        }
    }
    if let Some(deadline) = ctx.deadline {
        if Instant::now() >= deadline {
            ctx.aborted = true;
        }
    }
}

/// Whether `mv`, the `move_index`'th move tried at this node (0-based,
/// in `order_moves`'s ranking), is safe to search at a reduced depth
/// under late-move reductions. Deliberately conservative: excludes
/// anything already prioritized by move ordering (TT move, noisy
/// moves, killers) or that arrived early despite being quiet, requires
/// meaningful remaining depth, and refuses to reduce *any* move --
/// quiet or not -- whenever either player currently faces an immediate
/// winning threat, since that signals a tactically sharp position
/// where a shallow trial search is more likely to misjudge a quiet
/// move's value.
fn is_lmr_eligible<G: Game, H: SearchHooks<G>>(
    hooks: &H,
    state: &G::State,
    mv: &G::Move,
    depth: u8,
    move_index: usize,
    tt_move: Option<G::Move>,
    killers: [Option<G::Move>; 2],
) -> bool {
    if depth < LMR_MIN_DEPTH || move_index < LMR_MIN_MOVE_INDEX {
        return false;
    }
    if tt_move == Some(*mv) || hooks.move_features(state, mv).is_noisy {
        return false;
    }
    if killers[0] == Some(*mv) || killers[1] == Some(*mv) {
        return false;
    }
    let a = G::current_player(state);
    let b = G::other_player(a);
    if hooks.has_immediate_threat(state, a) || hooks.has_immediate_threat(state, b) {
        return false;
    }
    true
}

/// Cheap best-guess ordering for quiescence's move list: immediate wins
/// first (they end the search outright), then any other noisy move in
/// generation order. Doesn't touch `ctx.killers`/`ctx.history` — reusing
/// `order_moves` here would risk indexing `ctx.killers[ply]` out of
/// bounds, since quiescence's `ply` can run past `ctx.max_ply` (it has
/// its own, separate depth cap).
fn order_noisy_moves<G: Game, H: SearchHooks<G>>(hooks: &H, moves: &mut [G::Move], state: &G::State) {
    let score = |mv: &G::Move| -> i32 {
        if hooks.move_features(state, mv).priority == MovePriority::ImmediateWin { 1 } else { 0 }
    };
    moves.sort_by_key(|mv| std::cmp::Reverse(score(mv)));
}

/// Extends search past the normal iterative-deepening horizon along
/// "noisy" lines only, to avoid the horizon effect: a plain static
/// evaluation right before an obvious recapture would misjudge it as a
/// clean material gain when it's actually an even trade. Never touches
/// `ctx.tt` — a quiescence node's score reflects only a partial
/// (noisy-moves-only) search, not a real minimax value at any depth,
/// so it isn't safe to cache in the same table normal search results
/// share.
///
/// "Stand pat" (the position's plain static eval, used as a lower bound
/// alpha can never fall below) is a chess-search convention carried over
/// as a heuristic approximation here, not a literal model of any
/// specific game's forced-move rule.
fn quiescence<G: Game, H: SearchHooks<G>>(
    hooks: &H,
    ctx: &mut SearchContext<G>,
    state: &G::State,
    mut alpha: i32,
    beta: i32,
    ply: usize,
    extra_ply_remaining: u8,
) -> Option<i32> {
    check_limits(ctx);
    if ctx.aborted {
        return None;
    }
    ctx.nodes += 1;
    ctx.quiescence_nodes += 1;

    match G::result(state) {
        GameResult::Win(winner) => {
            let score = MATE - ply as i32;
            return Some(if winner == G::current_player(state) { score } else { -score });
        }
        GameResult::Draw => return Some(0),
        GameResult::InProgress => {}
    }

    let key = G::position_key(state);
    if ctx.path.contains(&key) {
        return Some(0);
    }

    let stand_pat = hooks.evaluate(state);
    if stand_pat >= beta {
        return Some(stand_pat);
    }
    if stand_pat > alpha {
        alpha = stand_pat;
    }
    if extra_ply_remaining == 0 || ply >= ctx.max_ply {
        return Some(stand_pat);
    }

    let mut noisy: Vec<G::Move> =
        G::legal_moves(state).into_iter().filter(|mv| hooks.move_features(state, mv).is_noisy).collect();
    if noisy.is_empty() {
        return Some(stand_pat);
    }
    order_noisy_moves(hooks, &mut noisy, state);

    ctx.path.push(key);
    let mut best = stand_pat;
    let mut was_aborted = false;
    for mv in noisy {
        let child = G::apply_move(state, mv);
        let child_score = match quiescence(hooks, ctx, &child, -beta, -alpha, ply + 1, extra_ply_remaining - 1) {
            Some(s) => -s,
            None => {
                was_aborted = true;
                break;
            }
        };
        if child_score > best {
            best = child_score;
        }
        if best > alpha {
            alpha = best;
        }
        if alpha >= beta {
            break;
        }
    }
    ctx.path.pop();

    if was_aborted {
        return None;
    }
    Some(best)
}

/// Orders `moves` best-guess-first: the transposition table's previous
/// best move, then immediate wins, then any other capture, then killer
/// moves (quiet moves that caused a beta cutoff at this ply in a
/// sibling branch, if `use_killers`), then history heuristic score (if
/// `use_history`). A pass (if the game has one) sorts last — legal
/// only when nothing else is available, never worth trying first.
/// Disabling either heuristic here only affects ordering, never which
/// moves are searched, so it cannot change a completed search's score
/// — only how many nodes it costs to get there.
#[allow(clippy::too_many_arguments)]
fn order_moves<G: Game, H: SearchHooks<G>>(
    hooks: &H,
    moves: &mut [G::Move],
    state: &G::State,
    tt_move: Option<G::Move>,
    killers: [Option<G::Move>; 2],
    history: &[i32],
    use_killers: bool,
    use_history: bool,
    use_cached_key: bool,
) {
    let score = |mv: &G::Move| -> i32 {
        if tt_move == Some(*mv) {
            return 1_000_000;
        }
        let features = hooks.move_features(state, mv);
        match features.priority {
            MovePriority::ImmediateWin => 900_000,
            MovePriority::Capture => 500_000,
            MovePriority::Ordinary => {
                if use_killers && (killers[0] == Some(*mv) || killers[1] == Some(*mv)) {
                    100_000
                } else if use_history {
                    features.history_bucket.map(|b| history[b]).unwrap_or(0)
                } else {
                    0
                }
            }
            MovePriority::Pass => -1_000_000,
        }
    };
    // `sort_by_key` re-invokes its key closure on every comparison the
    // sort performs (O(n log n) calls), not once per element. For this
    // closure that means redundant work at comparison-count rate rather
    // than move-count rate; `sort_by_cached_key` computes each element's
    // key exactly once into a temporary buffer, then sorts using the
    // cached keys, while remaining stable and producing the identical
    // order `sort_by_key` would have. The `use_cached_key` branch exists
    // only so callers can A/B benchmark the two.
    if use_cached_key {
        moves.sort_by_cached_key(|mv| std::cmp::Reverse(score(mv)));
    } else {
        moves.sort_by_key(|mv| std::cmp::Reverse(score(mv)));
    }
}

/// Root-only move ordering when a root-policy evaluator is attached and
/// produced a validated policy (see `root_policy_is_usable`): immediate
/// wins first -- specifically the same narrower "immediate win" signal
/// `order_root_moves` always used (`priority == ImmediateWin && !
/// is_capture`, i.e. excluding a win that also happens to capture
/// something -- a capture that wins outright still gets ranked purely
/// by the policy/TT-move tiers below, exactly as the original engine
/// did; not "cleaned up" to include it) -- then a TT-recommended move
/// if one exists and is actually legal here, then every remaining move
/// ranked purely by root-policy probability. Deliberately *not*
/// classical capture-first priority for this last tier — captures are
/// ranked by the policy exactly like any other move, so its own read on
/// capture-vs-quiet value at the root can actually influence the
/// search. Below the root, ordering is entirely classical; see
/// `order_moves`.
///
/// Small integer-ish sentinels (not `f64::MAX`/`MIN`) for the top two
/// tiers: `f64::MAX - 1.0` rounds back to `f64::MAX` at that magnitude
/// (nowhere near enough mantissa precision to represent a difference of
/// 1.0), which would silently collapse the immediate-win and TT-move
/// tiers into ties. `3.0`/`2.0` sit safely above any softmax probability
/// (always in `[0, 1]`) with no such risk.
fn order_root_moves<G: Game, H: SearchHooks<G>>(
    hooks: &H,
    moves: &mut [G::Move],
    state: &G::State,
    tt_move: Option<G::Move>,
    root_policy: &[(G::Move, f32)],
) {
    let score = |mv: &G::Move| -> f64 {
        let features = hooks.move_features(state, mv);
        if features.priority == MovePriority::ImmediateWin && !features.is_capture {
            return 3.0;
        }
        if tt_move == Some(*mv) {
            return 2.0;
        }
        root_policy.iter().find(|(policy_mv, _)| policy_mv == mv).map(|(_, p)| *p as f64).unwrap_or(0.0)
    };
    moves.sort_by(|a, b| {
        score(b).partial_cmp(&score(a)).expect("root_policy is validated finite by root_policy_is_usable before use")
    });
}

/// Whether `policy` is safe to use for root ordering: exactly the same
/// moves as `legal` (as a multiset — same length, every policy move
/// found in `legal` with none left over, so neither duplicates nor
/// extras nor omissions slip through), and every probability finite (no
/// NaN/Inf). Any evaluator implementation that fails this causes a
/// clean fallback to fully classical root ordering, never a panic or a
/// silently wrong move.
fn root_policy_is_usable<G: Game>(policy: &[(G::Move, f32)], legal: &[G::Move]) -> bool {
    if policy.len() != legal.len() {
        return false;
    }
    if !policy.iter().all(|(_, p)| p.is_finite()) {
        return false;
    }
    let mut remaining: Vec<G::Move> = legal.to_vec();
    for (mv, _) in policy {
        match remaining.iter().position(|m| m == mv) {
            Some(pos) => {
                remaining.swap_remove(pos);
            }
            None => return false,
        }
    }
    remaining.is_empty()
}

/// Mate scores (`MATE - ply_at_terminal`) are relative to how deep into
/// *this particular search* the terminal node was found — but the TT
/// stores results keyed by position, to be reused from whatever ply a
/// later probe happens to reach that position at. Any score at least
/// this close to `MATE` is unambiguously a mate score (heuristic eval
/// magnitudes top out far below this for any reasonable evaluator), so
/// it's safe to convert at the TT boundary without a per-search
/// threshold.
const MATE_THRESHOLD: i32 = MATE - 1000;

/// Converts a ply-relative score (as used throughout the live search) to
/// the ply-independent form stored in the TT ("mate in k from this
/// position", not "mate in k from wherever the root happened to be").
fn score_to_tt(score: i32, ply: usize) -> i32 {
    if score >= MATE_THRESHOLD {
        score + ply as i32
    } else if score <= -MATE_THRESHOLD {
        score - ply as i32
    } else {
        score
    }
}

/// Inverse of `score_to_tt`: converts a stored ply-independent score back
/// to what it means at the *current* node's ply. Not called in
/// production now that TT scores are advisory-only and never read back
/// (see `TtEntry::score`), but kept and unit-tested (round-tripping
/// against `score_to_tt`) since a future path-aware TT would need it.
#[allow(dead_code)]
fn score_from_tt(score: i32, ply: usize) -> i32 {
    if score >= MATE_THRESHOLD {
        score - ply as i32
    } else if score <= -MATE_THRESHOLD {
        score + ply as i32
    } else {
        score
    }
}

/// Negamax alpha-beta over `state` to `depth` plies. Returns `None` if
/// the search was aborted (node/time limit hit) partway through —
/// callers must discard the whole result, not just this call's return
/// value, since an aborted subtree's score is meaningless.
///
/// Returns `(score, best_move, path_dependent)`. `best_move` is `None`
/// for every early-return path (terminal, repetition, depth cutoff, TT
/// cutoff), since those don't run the move loop; it's always `Some` for
/// a node that completes the move loop without aborting. The root call
/// in `analyze` reads it directly instead of round-tripping through the
/// transposition table, so root-move selection works correctly even
/// with `use_tt: false`.
///
/// `path_dependent` is true if this node's value depended on the
/// repetition check (`ctx.path`) — either directly (this node *is* a
/// repeated ancestor) or transitively (some explored child was
/// path-dependent). A repetition score of 0 is only valid because of
/// which ancestors happen to be on *this specific* search line; the same
/// position reached via different ancestors might not repeat at all. So
/// a path-dependent node's result must never be written to the TT, or a
/// later, unrelated search could incorrectly inherit a draw score that
/// only ever applied to one particular path.
#[allow(clippy::too_many_arguments)]
fn negamax<G: Game, H: SearchHooks<G>>(
    hooks: &H,
    ctx: &mut SearchContext<G>,
    state: &G::State,
    depth: u8,
    mut alpha: i32,
    beta: i32,
    ply: usize,
) -> Option<(i32, Option<G::Move>, bool)> {
    check_limits(ctx);
    if ctx.aborted {
        return None;
    }
    ctx.nodes += 1;

    match G::result(state) {
        GameResult::Win(winner) => {
            let score = MATE - ply as i32;
            return Some((if winner == G::current_player(state) { score } else { -score }, None, false));
        }
        GameResult::Draw => return Some((0, None, false)),
        GameResult::InProgress => {}
    }

    let key = G::position_key(state);
    if ctx.path.contains(&key) {
        return Some((0, None, true));
    }

    if ply >= ctx.max_ply {
        // Hard safety cutoff, independent of quiescence: never exceed the
        // configured absolute ply bound, regardless of search mode.
        return Some((hooks.evaluate(state), None, false));
    }
    if depth == 0 {
        // The normal iterative-deepening horizon — the only point
        // quiescence activates from.
        let score = match ctx.quiescence_max_extra_ply {
            Some(extra_ply) => quiescence(hooks, ctx, state, alpha, beta, ply, extra_ply)?,
            None => hooks.evaluate(state),
        };
        return Some((score, None, false));
    }

    let orig_alpha = alpha;
    // TT entries are advisory-only: `entry.best_move` seeds move ordering
    // (an old best guess can only make search faster or slower to prove
    // the same result), but a cached score is *never* used to return
    // early or tighten alpha/beta -- see this function's own doc comment
    // on `path_dependent` for why (the Graph History Interaction
    // problem).
    let mut tt_move = None;
    if ctx.use_tt {
        if let Some(entry) = ctx.tt.probe(key) {
            ctx.tt_hits += 1;
            tt_move = entry.best_move;
        }
    }

    let mut moves = G::legal_moves(state);
    let killers_at_ply = ctx.killers[ply];
    match (ply == 0, &ctx.root_policy) {
        (true, Some(root_policy)) => order_root_moves(hooks, &mut moves, state, tt_move, root_policy),
        _ => order_moves(
            hooks,
            &mut moves,
            state,
            tt_move,
            killers_at_ply,
            &ctx.history,
            ctx.killer_moves,
            ctx.history_heuristic,
            ctx.order_moves_use_cached_key,
        ),
    }

    ctx.path.push(key);

    let mut best_score = i32::MIN + 1;
    let mut best_move: Option<G::Move> = None;
    let mut was_aborted = false;
    let mut path_dependent = false;
    for (i, mv) in moves.iter().enumerate() {
        let child = G::apply_move(state, *mv);
        let lmr_eligible = ctx.lmr && is_lmr_eligible(hooks, state, mv, depth, i, tt_move, killers_at_ply);
        let child_score = if lmr_eligible {
            // LMR: a conservative subset of quiet, late moves gets tried
            // at a reduced depth and the *full* window first — cheap to
            // confirm "no, this doesn't beat alpha" (the common case,
            // same rationale as PVS's null-window scout, but shrinking
            // depth instead of the window). Unlike PVS's scout, a
            // reduced-depth search can theoretically MISS real strength
            // a full-depth search would have found, so a result that
            // does beat alpha isn't trusted as-is — it's always
            // re-searched at full depth before being accepted.
            ctx.lmr_reductions += 1;
            let reduced = match negamax(hooks, ctx, &child, depth - 1 - LMR_REDUCTION, -beta, -alpha, ply + 1) {
                Some((s, _, tainted)) => {
                    if tainted {
                        path_dependent = true;
                    }
                    -s
                }
                None => {
                    was_aborted = true;
                    break;
                }
            };
            if reduced > alpha {
                ctx.lmr_researches += 1;
                match negamax(hooks, ctx, &child, depth - 1, -beta, -alpha, ply + 1) {
                    Some((s, _, tainted)) => {
                        if tainted {
                            path_dependent = true;
                        }
                        -s
                    }
                    None => {
                        was_aborted = true;
                        break;
                    }
                }
            } else {
                reduced
            }
        } else {
            // PVS: every move after the first is assumed worse by move
            // ordering, so probe it with a zero-width window first --
            // cheap to prove "no, this doesn't beat alpha" (the common
            // case) and only worth a full-window re-search when the
            // scout disagrees. At an already-null-window node
            // (beta - alpha <= 1, i.e. we ourselves are some ancestor's
            // scout), `scout > alpha` and `scout < beta` can never both
            // hold for integer scores, so this never re-searches there
            // -- no special-casing needed.
            let use_scout = ctx.pvs && i > 0;
            let scout_score = if use_scout {
                match negamax(hooks, ctx, &child, depth - 1, -alpha - 1, -alpha, ply + 1) {
                    Some((s, _, tainted)) => {
                        if tainted {
                            path_dependent = true;
                        }
                        Some(-s)
                    }
                    None => {
                        was_aborted = true;
                        break;
                    }
                }
            } else {
                None
            };
            let needs_full_search = match scout_score {
                Some(s) => s > alpha && s < beta,
                None => true,
            };
            if needs_full_search {
                if scout_score.is_some() {
                    ctx.pvs_researches += 1;
                }
                match negamax(hooks, ctx, &child, depth - 1, -beta, -alpha, ply + 1) {
                    Some((s, _, tainted)) => {
                        if tainted {
                            path_dependent = true;
                        }
                        -s
                    }
                    None => {
                        was_aborted = true;
                        break;
                    }
                }
            } else {
                scout_score.expect("needs_full_search is only false when scout_score is Some")
            }
        };
        if child_score > best_score {
            best_score = child_score;
            best_move = Some(*mv);
        }
        if best_score > alpha {
            alpha = best_score;
        }
        if alpha >= beta {
            ctx.beta_cutoffs += 1;
            // Matches the original engine's exact gate: keyed on
            // `is_capture` alone, not `is_noisy`/`priority` -- so a
            // non-capturing immediate win still becomes a killer/gets a
            // history entry here, same as before.
            let features = hooks.move_features(state, mv);
            if !features.is_capture {
                if ctx.killer_moves {
                    let slot = &mut ctx.killers[ply];
                    if slot[0] != Some(*mv) {
                        slot[1] = slot[0];
                        slot[0] = Some(*mv);
                    }
                }
                if ctx.history_heuristic {
                    if let Some(bucket) = features.history_bucket {
                        ctx.history[bucket] += ctx.history_bonus.value(depth);
                    }
                }
            }
            break;
        }
    }

    ctx.path.pop();

    if was_aborted {
        return None;
    }

    if ctx.use_tt && !path_dependent {
        let bound = if best_score <= orig_alpha {
            Bound::Upper
        } else if best_score >= beta {
            Bound::Lower
        } else {
            Bound::Exact
        };
        ctx.tt.store(key, depth, bound, score_to_tt(best_score, ply), best_move);
    }

    Some((best_score, best_move, path_dependent))
}

fn reconstruct_pv<G: Game>(tt: &TranspositionTable<G>, root: &G::State, max_len: u8) -> Vec<G::Move> {
    let mut pv = Vec::new();
    let mut state = *root;
    for _ in 0..max_len {
        let Some(mv) = tt.probe(G::position_key(&state)).and_then(|e| e.best_move) else {
            break;
        };
        pv.push(mv);
        state = G::apply_move(&state, mv);
        if matches!(G::result(&state), GameResult::Win(..) | GameResult::Draw) {
            break;
        }
    }
    pv
}

/// Deterministic alpha-beta opponent: iterative-deepening negamax with a
/// bounded transposition table, killer/history move ordering, and
/// path-repetition detection.
pub struct AlphaBetaPlayer<G: Game, H: SearchHooks<G>> {
    pub config: AlphaBetaConfig,
    pub hooks: H,
    tt: TranspositionTable<G>,
}

impl<G: Game, H: SearchHooks<G>> AlphaBetaPlayer<G, H> {
    pub fn new(config: AlphaBetaConfig, hooks: H) -> Self {
        let tt = TranspositionTable::new(config.tt_megabytes);
        AlphaBetaPlayer { config, hooks, tt }
    }

    /// Clears search work retained from a previous game while preserving
    /// configuration and hooks. Call this at game boundaries; the table
    /// should remain live between moves within one game.
    pub fn reset_for_new_game(&mut self) {
        self.tt = TranspositionTable::new(self.config.tt_megabytes);
    }

    /// Runs iterative deepening from depth 1 up to whatever `self.config`
    /// allows, returning the last depth that completed *without* being
    /// aborted by a node/time limit. A single-legal-move position is
    /// answered immediately with no search. `root_policy_evaluator`,
    /// if given, is called at most once per move (see
    /// `AlphaBetaAnalysis::root_policy_calls`) -- its value output is
    /// read and immediately discarded; only its policy feeds root
    /// ordering, never node scores.
    pub fn analyze(
        &mut self,
        state: &G::State,
        root_policy_evaluator: Option<&dyn RootPolicyEvaluator<G>>,
    ) -> AlphaBetaAnalysis<G> {
        let start = Instant::now();
        let deadline = match self.config.limit {
            SearchLimit::MoveTime(d) => Some(start + d),
            _ => None,
        };
        let node_limit = match self.config.limit {
            SearchLimit::Nodes(n) => Some(n),
            _ => None,
        };
        let max_depth_limit = match self.config.limit {
            SearchLimit::Depth(d) => Some(d),
            _ => None,
        };

        let legal = G::legal_moves(state);
        if legal.len() == 1 {
            return AlphaBetaAnalysis {
                best_move: legal[0],
                score: 0,
                completed_depth: 0,
                selective_depth: 0,
                nodes: 0,
                quiescence_nodes: 0,
                tt_hits: 0,
                beta_cutoffs: 0,
                pvs_researches: 0,
                aspiration_researches: 0,
                lmr_reductions: 0,
                lmr_researches: 0,
                // A single legal move never calls the evaluator at all —
                // there is nothing to order.
                root_policy_calls: 0,
                root_policy_used: false,
                root_policy_time: Duration::ZERO,
                elapsed: start.elapsed(),
                principal_variation: vec![legal[0]],
            };
        }

        if matches!(self.config.limit, SearchLimit::Depth(0)) {
            // A literal zero-ply search: statically evaluate each legal
            // move's resulting position with no recursion at all.
            return self.analyze_depth_zero(state, &legal, start);
        }

        // One evaluator call per real move, before iterative deepening
        // starts, cached in `ctx.root_policy` for every depth this call
        // searches — never re-requested per depth. Timed with the same
        // clock `start`/`elapsed` use, so a slow evaluator call is
        // counted against this move's budget like everything else.
        let mut root_policy_calls = 0u64;
        let mut root_policy_time = Duration::ZERO;
        let root_policy: Option<Vec<(G::Move, f32)>> = root_policy_evaluator.and_then(|evaluator| {
            let call_start = Instant::now();
            let (policy, _value) = evaluator.evaluate(state);
            root_policy_time = call_start.elapsed();
            root_policy_calls = 1;
            root_policy_is_usable::<G>(&policy, &legal).then_some(policy)
        });
        let root_policy_used = root_policy.is_some();

        self.tt.new_generation();

        let mut ctx = SearchContext {
            tt: std::mem::replace(&mut self.tt, TranspositionTable::new(1)),
            killers: vec![[None; 2]; self.config.max_ply + 1],
            history: vec![0; H::HISTORY_BUCKETS],
            nodes: 0,
            quiescence_nodes: 0,
            tt_hits: 0,
            beta_cutoffs: 0,
            pvs_researches: 0,
            lmr_reductions: 0,
            lmr_researches: 0,
            quiescence_max_extra_ply: self.config.quiescence_max_extra_ply,
            pvs: self.config.pvs,
            killer_moves: self.config.killer_moves,
            history_heuristic: self.config.history_heuristic,
            history_bonus: self.config.history_bonus,
            lmr: self.config.lmr,
            order_moves_use_cached_key: self.config.order_moves_use_cached_key,
            root_policy,
            node_limit,
            deadline,
            aborted: false,
            path: Vec::with_capacity(self.config.max_ply + 1),
            max_ply: self.config.max_ply,
            use_tt: self.config.tt_megabytes > 0,
        };

        let mut best = AlphaBetaAnalysis {
            best_move: legal[0],
            score: 0,
            completed_depth: 0,
            selective_depth: 0,
            nodes: 0,
            quiescence_nodes: 0,
            tt_hits: 0,
            beta_cutoffs: 0,
            pvs_researches: 0,
            aspiration_researches: 0,
            lmr_reductions: 0,
            lmr_researches: 0,
            root_policy_calls,
            root_policy_used,
            root_policy_time,
            elapsed: Duration::ZERO,
            principal_variation: Vec::new(),
        };

        let mut aspiration_researches = 0u64;
        let mut depth: u8 = 1;
        loop {
            ctx.aborted = false;
            // Aspiration windows only apply from the second iteration
            // onward — depth 1 has no previous score to center a window
            // on, so it always searches the full range.
            let result = match self.config.aspiration_window {
                Some(window) if depth >= 2 => {
                    let alpha = best.score.saturating_sub(window).max(-MATE - 1);
                    let beta = best.score.saturating_add(window).min(MATE + 1);
                    match negamax(&self.hooks, &mut ctx, state, depth, alpha, beta, 0) {
                        Some((score, mv, tainted)) if score > alpha && score < beta => Some((score, mv, tainted)),
                        Some(_) => {
                            // Fail-low or fail-high: the narrow window only
                            // proves a bound, not the exact score — a
                            // full-window re-search is needed for a result
                            // this depth can actually be trusted to report.
                            aspiration_researches += 1;
                            negamax(&self.hooks, &mut ctx, state, depth, -MATE - 1, MATE + 1, 0)
                        }
                        None => None,
                    }
                }
                _ => negamax(&self.hooks, &mut ctx, state, depth, -MATE - 1, MATE + 1, 0),
            };
            let Some((score, root_move, _)) = result else {
                break; // aborted mid-depth: discard, keep the previous depth's `best`
            };
            let mv = root_move.unwrap_or(best.best_move);
            best = AlphaBetaAnalysis {
                best_move: mv,
                score,
                completed_depth: depth,
                selective_depth: depth,
                nodes: ctx.nodes,
                quiescence_nodes: ctx.quiescence_nodes,
                tt_hits: ctx.tt_hits,
                beta_cutoffs: ctx.beta_cutoffs,
                pvs_researches: ctx.pvs_researches,
                aspiration_researches,
                lmr_reductions: ctx.lmr_reductions,
                lmr_researches: ctx.lmr_researches,
                root_policy_calls,
                root_policy_used,
                root_policy_time,
                elapsed: start.elapsed(),
                principal_variation: reconstruct_pv(&ctx.tt, state, depth),
            };

            if score.abs() >= MATE - ctx.max_ply as i32 {
                // Found a forced mate within the searchable horizon —
                // deepening further can't change the outcome, only the
                // reported depth.
                break;
            }
            depth += 1;
            if let Some(d) = max_depth_limit {
                if depth > d {
                    break;
                }
            }
            if depth as usize > self.config.max_ply {
                break;
            }
            if let Some(n) = node_limit {
                if ctx.nodes >= n {
                    break;
                }
            }
            if let Some(dl) = deadline {
                if Instant::now() >= dl {
                    break;
                }
            }
        }

        best.nodes = ctx.nodes; // includes any aborted final iteration's partial work
        best.quiescence_nodes = ctx.quiescence_nodes;
        best.tt_hits = ctx.tt_hits; // same reasoning — keep hit-rate math consistent with `nodes`
        best.beta_cutoffs = ctx.beta_cutoffs;
        best.pvs_researches = ctx.pvs_researches;
        best.lmr_reductions = ctx.lmr_reductions;
        best.lmr_researches = ctx.lmr_researches;
        best.elapsed = start.elapsed();
        self.tt = ctx.tt;
        best
    }

    fn analyze_depth_zero(&self, state: &G::State, legal: &[G::Move], start: Instant) -> AlphaBetaAnalysis<G> {
        let mut best_score = i32::MIN;
        let mut best_move = legal[0];
        for &mv in legal {
            let child = G::apply_move(state, mv);
            let score = match G::result(&child) {
                GameResult::Win(winner) => {
                    if winner == G::current_player(state) {
                        MATE - 1
                    } else {
                        -(MATE - 1)
                    }
                }
                GameResult::Draw => 0,
                GameResult::InProgress => -self.hooks.evaluate(&child),
            };
            if score > best_score {
                best_score = score;
                best_move = mv;
            }
        }
        AlphaBetaAnalysis {
            best_move,
            score: best_score,
            completed_depth: 0,
            selective_depth: 0,
            nodes: legal.len() as u64,
            quiescence_nodes: 0,
            tt_hits: 0,
            beta_cutoffs: 0,
            pvs_researches: 0,
            aspiration_researches: 0,
            lmr_reductions: 0,
            lmr_researches: 0,
            // A literal zero-ply search never runs the iterative-
            // deepening loop that would consult a root-policy evaluator.
            root_policy_calls: 0,
            root_policy_used: false,
            root_policy_time: Duration::ZERO,
            elapsed: start.elapsed(),
            principal_variation: vec![best_move],
        }
    }
}

#[cfg(test)]
mod tests;
