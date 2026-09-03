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
