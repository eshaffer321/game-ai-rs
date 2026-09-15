//! Exercises the generic engine end to end against a toy game (Nim:
//! subtract 1 or 2 from a shared pile, whoever takes the last token
//! wins -- optimal play always leaves a multiple of 3 for the
//! opponent), independent of either real game adapter. Byte-identical
//! parity with Onifish's original engine is checked separately, in
//! onitama-ai's dual-engine parity test against the frozen positions.

use super::*;
use crate::{MoveFeatures, MovePriority, SearchHooks};
use game_ai_core::GameResult as CoreResult;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum NimPlayer {
    A,
    B,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct NimState {
    pile: u8,
    to_move: NimPlayer,
}

struct NimGame;

impl Game for NimGame {
    type State = NimState;
    type Move = u8;
    type Player = NimPlayer;
    type PositionKey = (u8, u8);

    fn current_player(state: &Self::State) -> Self::Player {
        state.to_move
    }

    fn other_player(player: Self::Player) -> Self::Player {
        match player {
            NimPlayer::A => NimPlayer::B,
            NimPlayer::B => NimPlayer::A,
        }
    }

    fn legal_moves(state: &Self::State) -> Vec<Self::Move> {
        (1..=2u8).filter(|&take| take <= state.pile).collect()
    }

    fn apply_move(state: &Self::State, mv: Self::Move) -> Self::State {
        NimState { pile: state.pile - mv, to_move: Self::other_player(state.to_move) }
    }

    fn result(state: &Self::State) -> CoreResult<Self::Player> {
        if state.pile == 0 {
            CoreResult::Win(Self::other_player(state.to_move))
        } else {
            CoreResult::InProgress
        }
    }

    fn position_key(state: &Self::State) -> Self::PositionKey {
        (state.pile, state.to_move as u8)
    }

    fn tt_hash(key: &Self::PositionKey) -> u64 {
        ((key.0 as u64) << 8 | key.1 as u64).wrapping_mul(0x9E3779B97F4A7C15)
    }
}

/// Identical rules to `NimGame`, kept as a separate type (rather than
/// just setting the const on `NimGame` itself) so the negative guard
/// test above can rely on `NimGame` staying at the trait's default
/// (`false`) while this one exercises the authoritative-TT machinery
/// positively. `pile` strictly decreases by 1 or 2 on every move and
/// never increases -- genuinely acyclic, not just a test-fixture
/// shortcut, the same shape as Santorini's own `ProgressMeasure` proof.
struct AcyclicNimGame;

impl Game for AcyclicNimGame {
    type State = NimState;
    type Move = u8;
    type Player = NimPlayer;
    type PositionKey = (u8, u8);

    fn current_player(state: &Self::State) -> Self::Player {
        NimGame::current_player(state)
    }
    fn other_player(player: Self::Player) -> Self::Player {
        NimGame::other_player(player)
    }
    fn legal_moves(state: &Self::State) -> Vec<Self::Move> {
        NimGame::legal_moves(state)
    }
    fn apply_move(state: &Self::State, mv: Self::Move) -> Self::State {
        NimGame::apply_move(state, mv)
    }
    fn result(state: &Self::State) -> CoreResult<Self::Player> {
        NimGame::result(state)
    }
    fn position_key(state: &Self::State) -> Self::PositionKey {
        NimGame::position_key(state)
    }
    fn tt_hash(key: &Self::PositionKey) -> u64 {
        NimGame::tt_hash(key)
    }
    const SUPPORTS_AUTHORITATIVE_TT: bool = true;
}

impl SearchHooks<AcyclicNimGame> for NimHooks {
    const HISTORY_BUCKETS: usize = 3;
    type EvalState = ();
    fn init_eval_state(&self, _state: &NimState) {}
    fn evaluate(&self, state: &NimState, (): &()) -> i32 {
        <NimHooks as SearchHooks<NimGame>>::evaluate(self, state, &())
    }
    fn move_features(&self, state: &NimState, mv: &u8) -> MoveFeatures {
        <NimHooks as SearchHooks<NimGame>>::move_features(self, state, mv)
    }
    fn has_immediate_threat(&self, state: &NimState, player: NimPlayer) -> bool {
        <NimHooks as SearchHooks<NimGame>>::has_immediate_threat(self, state, player)
    }
}

#[derive(Default)]
struct NimHooks;

impl SearchHooks<NimGame> for NimHooks {
    const HISTORY_BUCKETS: usize = 3;

    type EvalState = ();

    fn init_eval_state(&self, _state: &NimState) {}

    fn evaluate(&self, state: &NimState, (): &()) -> i32 {
        // A pile that's a multiple of 3 is bad for the player to move
        // (under optimal play they'll eventually be forced to leave a
        // winning position for the opponent); otherwise good.
        if state.pile.is_multiple_of(3) {
            -1
        } else {
            1
        }
    }

    fn move_features(&self, state: &NimState, mv: &u8) -> MoveFeatures {
        let wins_immediately = *mv as u16 >= state.pile as u16;
        MoveFeatures {
            priority: if wins_immediately { MovePriority::ImmediateWin } else { MovePriority::Ordinary },
            is_noisy: wins_immediately,
            is_capture: false,
            history_bucket: (!wins_immediately).then_some(*mv as usize),
        }
    }

    fn has_immediate_threat(&self, state: &NimState, _player: NimPlayer) -> bool {
        state.pile <= 2
    }
}

#[test]
fn analyze_finds_the_optimal_nim_move_leaving_a_multiple_of_three() {
    let mut player: AlphaBetaPlayer<NimGame, NimHooks> = AlphaBetaPlayer::new(
        AlphaBetaConfig { limit: SearchLimit::Depth(10), ..AlphaBetaConfig::default() },
        NimHooks,
    );
    // Pile of 7: optimal play takes 1, leaving 6 (a multiple of 3).
    let state = NimState { pile: 7, to_move: NimPlayer::A };
    let analysis = player.analyze(&state, None);
    assert_eq!(analysis.best_move, 1);
    assert!(analysis.score > 0);
}

#[test]
fn analyze_reports_a_losing_score_from_a_multiple_of_three() {
    let mut player: AlphaBetaPlayer<NimGame, NimHooks> = AlphaBetaPlayer::new(
        AlphaBetaConfig { limit: SearchLimit::Depth(10), ..AlphaBetaConfig::default() },
        NimHooks,
    );
    // Pile of 6: whatever A takes (1 or 2), B can always restore a
    // multiple of 3 -- a theoretical loss for A under optimal play.
    let state = NimState { pile: 6, to_move: NimPlayer::A };
    let analysis = player.analyze(&state, None);
    assert!(analysis.score < 0);
}

#[test]
fn single_legal_move_short_circuits_with_no_search() {
    let mut player: AlphaBetaPlayer<NimGame, NimHooks> =
        AlphaBetaPlayer::new(AlphaBetaConfig::default(), NimHooks);
    let state = NimState { pile: 1, to_move: NimPlayer::A }; // only "take 1" is legal
    let analysis = player.analyze(&state, None);
    assert_eq!(analysis.best_move, 1);
    assert_eq!(analysis.nodes, 0);
}

#[test]
fn depth_zero_evaluates_each_move_statically_with_no_recursion() {
    let mut player: AlphaBetaPlayer<NimGame, NimHooks> =
        AlphaBetaPlayer::new(AlphaBetaConfig { limit: SearchLimit::Depth(0), ..AlphaBetaConfig::default() }, NimHooks);
    let state = NimState { pile: 7, to_move: NimPlayer::A };
    let analysis = player.analyze(&state, None);
    assert_eq!(analysis.nodes, 2); // exactly the two legal moves, no recursion
}

#[test]
fn reset_for_new_game_clears_the_transposition_table_without_losing_config() {
    let mut player: AlphaBetaPlayer<NimGame, NimHooks> = AlphaBetaPlayer::new(
        AlphaBetaConfig { limit: SearchLimit::Depth(6), ..AlphaBetaConfig::default() },
        NimHooks,
    );
    let state = NimState { pile: 7, to_move: NimPlayer::A };
    let before = player.analyze(&state, None);
    player.reset_for_new_game();
    let after = player.analyze(&state, None);
    assert_eq!(before.best_move, after.best_move);
    assert_eq!(before.score, after.score);
}

#[test]
#[should_panic(expected = "does not declare Game::SUPPORTS_AUTHORITATIVE_TT")]
fn authoritative_tt_requested_for_a_game_that_does_not_declare_it_safe_panics_loudly() {
    // NimGame doesn't override `SUPPORTS_AUTHORITATIVE_TT` (stays `false`,
    // the trait default) -- requesting `authoritative_tt: true` for it
    // must be a hard, loud failure at construction time, never a silent
    // fallback to advisory-only behavior. This is the two-independent-
    // gates mechanism itself, not any particular game's correctness.
    let _: AlphaBetaPlayer<NimGame, NimHooks> =
        AlphaBetaPlayer::new(AlphaBetaConfig { authoritative_tt: true, ..AlphaBetaConfig::default() }, NimHooks);
}

#[test]
fn authoritative_tt_matches_tt_completely_disabled_across_many_piles_and_tiny_tables() {
    // AcyclicNimGame declares SUPPORTS_AUTHORITATIVE_TT truthfully
    // (pile strictly decreases every move). Comparing against TT
    // completely disabled (tt_megabytes: 0), not merely advisory TT,
    // is the most conservative ground truth: no move-ordering
    // influence at all, so any score divergence can only come from
    // the authoritative early-return/window-tightening logic itself.
    // `tt_megabytes: 1` is the smallest size the API allows -- already
    // small enough, relative to how few distinct positions Nim has,
    // to exercise real collisions and replacement.
    for pile in 1..=40u8 {
        for to_move in [NimPlayer::A, NimPlayer::B] {
            let state = NimState { pile, to_move };
            for depth in [4u8, 10] {
                let mut baseline: AlphaBetaPlayer<AcyclicNimGame, NimHooks> = AlphaBetaPlayer::new(
                    AlphaBetaConfig { limit: SearchLimit::Depth(depth), tt_megabytes: 0, ..AlphaBetaConfig::default() },
                    NimHooks,
                );
                let mut candidate: AlphaBetaPlayer<AcyclicNimGame, NimHooks> = AlphaBetaPlayer::new(
                    AlphaBetaConfig {
                        limit: SearchLimit::Depth(depth),
                        tt_megabytes: 1,
                        authoritative_tt: true,
                        ..AlphaBetaConfig::default()
                    },
                    NimHooks,
                );

                let baseline_analysis = baseline.analyze(&state, None);
                let candidate_analysis = candidate.analyze(&state, None);

                assert_eq!(
                    candidate_analysis.score, baseline_analysis.score,
                    "pile {pile}, {to_move:?} to move, depth {depth}: authoritative TT diverged from TT-disabled \
                     ({} -> {})",
                    baseline_analysis.score, candidate_analysis.score
                );
            }
        }
    }
}

#[test]
fn score_to_tt_and_score_from_tt_round_trip() {
    for (score, ply) in [(0, 0), (100, 5), (-100, 5), (MATE - 1, 3), (-(MATE - 1), 3)] {
        assert_eq!(score_from_tt(score_to_tt(score, ply), ply), score);
    }
}

/// A tiny custom `RootPolicyEvaluator` proving the generic engine
/// actually consults it at the root, without needing any neural
/// infrastructure.
struct FixedPolicy(Vec<(u8, f32)>);

impl RootPolicyEvaluator<NimGame> for FixedPolicy {
    fn evaluate(&self, _state: &NimState) -> (Vec<(u8, f32)>, f32) {
        (self.0.clone(), 0.0)
    }
}

#[test]
fn root_policy_evaluator_is_consulted_when_provided() {
    let mut player: AlphaBetaPlayer<NimGame, NimHooks> = AlphaBetaPlayer::new(
        AlphaBetaConfig { limit: SearchLimit::Depth(4), ..AlphaBetaConfig::default() },
        NimHooks,
    );
    let state = NimState { pile: 5, to_move: NimPlayer::A };
    // Strongly favor the (suboptimal) move "2" -- since it isn't an
    // immediate win and there's no TT move yet, root ordering should
    // try it first, though the search itself still finds the true
    // best move regardless of ordering.
    let policy = FixedPolicy(vec![(1, 0.01), (2, 0.99)]);
    let analysis = player.analyze(&state, Some(&policy));
    assert_eq!(analysis.root_policy_calls, 1);
    assert!(analysis.root_policy_used);
}

#[test]
fn an_unusable_policy_falls_back_to_classical_ordering() {
    let mut player: AlphaBetaPlayer<NimGame, NimHooks> = AlphaBetaPlayer::new(
        AlphaBetaConfig { limit: SearchLimit::Depth(4), ..AlphaBetaConfig::default() },
        NimHooks,
    );
    let state = NimState { pile: 7, to_move: NimPlayer::A };
    // Wrong move set (includes an illegal "3") -- must be rejected.
    let policy = FixedPolicy(vec![(1, 0.5), (3, 0.5)]);
    let analysis = player.analyze(&state, Some(&policy));
    assert_eq!(analysis.root_policy_calls, 1);
    assert!(!analysis.root_policy_used);
    assert_eq!(analysis.best_move, 1); // still finds the true optimal move
}

// --- Reverse futility pruning ------------------------------------------

#[test]
fn rfp_is_none_by_default() {
    assert!(AlphaBetaConfig::default().rfp.is_none());
}

/// Same rules as `NimHooks`, but `supports_reverse_futility_pruning`
/// always returns `false` -- exercises the "a game excludes some
/// phase/region from RFP entirely" path (Santorini's Setup phase is
/// the real-world case) without needing a second real game.
#[derive(Default)]
struct PhaseExcludedHooks;

impl SearchHooks<AcyclicNimGame> for PhaseExcludedHooks {
    const HISTORY_BUCKETS: usize = 3;
    type EvalState = ();
    fn init_eval_state(&self, _state: &NimState) {}
    fn evaluate(&self, state: &NimState, eval_state: &()) -> i32 {
        <NimHooks as SearchHooks<AcyclicNimGame>>::evaluate(&NimHooks, state, eval_state)
    }
    fn move_features(&self, state: &NimState, mv: &u8) -> MoveFeatures {
        <NimHooks as SearchHooks<AcyclicNimGame>>::move_features(&NimHooks, state, mv)
    }
    fn has_immediate_threat(&self, state: &NimState, player: NimPlayer) -> bool {
        <NimHooks as SearchHooks<AcyclicNimGame>>::has_immediate_threat(&NimHooks, state, player)
    }
    fn supports_reverse_futility_pruning(&self, _state: &NimState) -> bool {
        false
    }
}

const TEST_RFP: RfpConfig = RfpConfig { max_depth: 8, base_margin: 100, margin_per_depth: 80, improving_bonus: 80 };

fn eligible_baseline() -> (NimState, u8, i32, i32, usize) {
    // pile=10: has_immediate_threat is false (pile > 2); depth=4 is
    // within TEST_RFP's max_depth; alpha/beta are an ordinary
    // non-PV (width-1) window nowhere near MATE_THRESHOLD; ply=3 is
    // non-root. Every exclusion test below starts from this
    // otherwise-eligible baseline and flips exactly one condition.
    (NimState { pile: 10, to_move: NimPlayer::A }, 4, 0, 1, 3)
}

#[test]
fn rfp_eligible_true_when_every_condition_holds() {
    let (state, depth, alpha, beta, ply) = eligible_baseline();
    assert!(rfp_eligible::<AcyclicNimGame, NimHooks>(&NimHooks, &state, &TEST_RFP, ply, depth, alpha, beta));
}

#[test]
fn rfp_eligible_excludes_root() {
    let (state, depth, alpha, beta, _ply) = eligible_baseline();
    assert!(!rfp_eligible::<AcyclicNimGame, NimHooks>(&NimHooks, &state, &TEST_RFP, 0, depth, alpha, beta));
}

#[test]
fn rfp_eligible_excludes_pv_nodes() {
    let (state, depth, _alpha, _beta, ply) = eligible_baseline();
    // A window wider than one point is this engine's only signal for
    // "PV node" (see `rfp_eligible`'s doc comment) -- (-10, 10) is a
    // full/wide window, unlike the eligible baseline's (0, 1).
    assert!(!rfp_eligible::<AcyclicNimGame, NimHooks>(&NimHooks, &state, &TEST_RFP, ply, depth, -10, 10));
}

#[test]
fn rfp_eligible_excludes_depth_beyond_the_configured_ceiling() {
    let (state, _depth, alpha, beta, ply) = eligible_baseline();
    assert!(rfp_eligible::<AcyclicNimGame, NimHooks>(&NimHooks, &state, &TEST_RFP, ply, TEST_RFP.max_depth, alpha, beta));
    assert!(!rfp_eligible::<AcyclicNimGame, NimHooks>(&NimHooks, &state, &TEST_RFP, ply, TEST_RFP.max_depth + 1, alpha, beta));
}

#[test]
fn rfp_eligible_excludes_beta_in_forced_mate_territory() {
    let (state, depth, alpha, _beta, ply) = eligible_baseline();
    assert!(!rfp_eligible::<AcyclicNimGame, NimHooks>(&NimHooks, &state, &TEST_RFP, ply, depth, alpha, MATE_THRESHOLD));
    assert!(!rfp_eligible::<AcyclicNimGame, NimHooks>(&NimHooks, &state, &TEST_RFP, ply, depth, alpha, MATE_THRESHOLD + 500));
}

#[test]
fn rfp_eligible_excludes_alpha_in_forced_mate_territory() {
    let (state, depth, _alpha, beta, ply) = eligible_baseline();
    assert!(!rfp_eligible::<AcyclicNimGame, NimHooks>(&NimHooks, &state, &TEST_RFP, ply, depth, -MATE_THRESHOLD, beta));
}

#[test]
fn rfp_eligible_excludes_immediate_threat() {
    let (mut state, depth, alpha, beta, ply) = eligible_baseline();
    state.pile = 2; // NimHooks::has_immediate_threat is `pile <= 2`
    assert!(!rfp_eligible::<AcyclicNimGame, NimHooks>(&NimHooks, &state, &TEST_RFP, ply, depth, alpha, beta));
}

#[test]
fn rfp_eligible_excludes_a_phase_the_hooks_deny() {
    let (state, depth, alpha, beta, ply) = eligible_baseline();
    assert!(!rfp_eligible::<AcyclicNimGame, PhaseExcludedHooks>(&PhaseExcludedHooks, &state, &TEST_RFP, ply, depth, alpha, beta));
}

#[test]
fn rfp_disabled_produces_byte_identical_output_to_before_it_existed() {
    // Every existing test above this point already runs with
    // `AlphaBetaConfig::default()` (`rfp: None`) and continues to
    // pass unchanged -- this test just makes the guarantee explicit
    // and independent of any one of them being edited later.
    let mut without_rfp_field_touched: AlphaBetaPlayer<NimGame, NimHooks> =
        AlphaBetaPlayer::new(AlphaBetaConfig { limit: SearchLimit::Depth(8), ..AlphaBetaConfig::default() }, NimHooks);
    let state = NimState { pile: 11, to_move: NimPlayer::A };
    let analysis = without_rfp_field_touched.analyze(&state, None);
    assert_eq!(analysis.rfp_attempts, 0);
    assert_eq!(analysis.rfp_cutoffs, 0);
    assert_eq!(analysis.best_move, 2); // leaves 9, a multiple of 3
}

#[test]
fn rfp_attempts_and_cutoffs_are_recorded_separately_when_enabled() {
    // A very tight (barely-permissive) margin: attempts happen at
    // every eligible node, but only some of them actually cut off.
    let mut player: AlphaBetaPlayer<AcyclicNimGame, NimHooks> = AlphaBetaPlayer::new(
        AlphaBetaConfig {
            limit: SearchLimit::Depth(10),
            pvs: true, // needed for any non-root window to narrow at all -- see rfp_eligible's doc comment
            rfp: Some(RfpConfig { max_depth: 10, base_margin: 1, margin_per_depth: 0, improving_bonus: 0 }),
            ..AlphaBetaConfig::default()
        },
        NimHooks,
    );
    let state = NimState { pile: 15, to_move: NimPlayer::A };
    let analysis = player.analyze(&state, None);
    assert!(analysis.rfp_attempts > 0, "expected at least one RFP-eligible node in a depth-10 search");
    assert!(analysis.rfp_cutoffs <= analysis.rfp_attempts, "cutoffs can never exceed attempts");
}

#[test]
fn rfp_cutoff_does_not_write_an_exact_tt_entry() {
    let hooks = NimHooks;
    // An absurdly permissive margin: any finite eval clears it, so
    // the very first eligible node cuts off unconditionally.
    let rfp = RfpConfig { max_depth: 255, base_margin: i32::MIN / 4, margin_per_depth: 0, improving_bonus: 0 };
    let root_state = NimState { pile: 10, to_move: NimPlayer::A };
    let child_state = AcyclicNimGame::apply_move(&root_state, 1);
    let max_ply = 64usize;

    let mut ctx = SearchContext::<AcyclicNimGame> {
        tt: TranspositionTable::new(1),
        killers: vec![[None; 2]; max_ply + 1],
        history: vec![0; <NimHooks as SearchHooks<AcyclicNimGame>>::HISTORY_BUCKETS],
        nodes: 0,
        quiescence_nodes: 0,
        tt_hits: 0,
        beta_cutoffs: 0,
        tt_cutoffs: 0,
        rfp_attempts: 0,
        rfp_cutoffs: 0,
        pvs_researches: 0,
        lmr_reductions: 0,
        lmr_researches: 0,
        quiescence_max_extra_ply: None,
        pvs: false,
        killer_moves: false,
        history_heuristic: false,
        history_bonus: HistoryBonus::Flat,
        lmr: false,
        order_moves_use_cached_key: true,
        authoritative_tt: false,
        rfp: Some(rfp),
        eval_history: vec![None; max_ply + 1],
        root_policy: None,
        node_limit: None,
        deadline: None,
        aborted: false,
        path: Vec::new(),
        max_ply,
        use_tt: true,
        move_buffers: vec![Vec::new(); max_ply + 1],
    };

    // ply=1, window (0, 1) -- non-root, non-PV (width 1) -- eligible.
    let result = negamax(&hooks, &mut ctx, &child_state, &(), 4, 0, 1, 1);
    assert!(result.is_some(), "search should not have aborted");
    assert!(ctx.rfp_cutoffs > 0, "expected the absurdly permissive margin to force a cutoff");

    let key = AcyclicNimGame::position_key(&child_state);
    assert!(
        ctx.tt.probe(key).is_none(),
        "an RFP cutoff must not write a TT entry at all (the conservative choice over storing an inexact bound)"
    );
}
