//! Exercises `SearchHooks` generically (no `dyn`) against a toy game,
//! independent of either real game adapter.

use game_ai_core::{Game, GameResult};

use crate::SearchHooks;

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

    fn result(state: &Self::State) -> GameResult<Self::Player> {
        if state.pile == 0 {
            GameResult::Win(Self::other_player(state.to_move))
        } else {
            GameResult::InProgress
        }
    }

    fn position_key(state: &Self::State) -> Self::PositionKey {
        (state.pile, state.to_move as u8)
    }

    fn tt_hash(key: &Self::PositionKey) -> u64 {
        ((key.0 as u64) << 8 | key.1 as u64).wrapping_mul(0x9E3779B97F4A7C15)
    }
}

/// Owns a runtime-configurable weight, the way Onitama's real hooks
/// own `EvalWeights` -- proving `SearchHooks` implementors can carry
/// state, unlike `Game`'s zero-sized marker types.
struct NimHooks {
    prefer_multiple_of: u8,
}

impl SearchHooks<NimGame> for NimHooks {
    fn evaluate(&self, state: &NimState) -> i32 {
        if state.pile.is_multiple_of(self.prefer_multiple_of) { -1 } else { 1 }
    }

    fn is_noisy(&self, state: &NimState, mv: &u8) -> bool {
        *mv as u16 >= state.pile as u16
    }

    fn has_immediate_threat(&self, state: &NimState, _player: NimPlayer) -> bool {
        state.pile <= 2
    }

    fn history_index(&self, mv: &u8) -> (usize, usize) {
        (*mv as usize, *mv as usize)
    }
}

fn run_generically<G: Game, H: SearchHooks<G>>(hooks: &H, state: &G::State) -> i32 {
    hooks.evaluate(state)
}

#[test]
fn search_hooks_dispatch_generically_with_no_dyn() {
    let hooks = NimHooks { prefer_multiple_of: 3 };
    let state = NimState { pile: 6, to_move: NimPlayer::A };
    assert_eq!(run_generically::<NimGame, _>(&hooks, &state), -1);
}

#[test]
fn is_noisy_flags_a_move_that_would_end_the_game() {
    let hooks = NimHooks { prefer_multiple_of: 3 };
    let state = NimState { pile: 2, to_move: NimPlayer::A };
    assert!(hooks.is_noisy(&state, &2));
    assert!(!hooks.is_noisy(&NimState { pile: 5, to_move: NimPlayer::A }, &1));
}

#[test]
fn has_immediate_threat_gates_on_a_small_pile() {
    let hooks = NimHooks { prefer_multiple_of: 3 };
    assert!(hooks.has_immediate_threat(&NimState { pile: 2, to_move: NimPlayer::A }, NimPlayer::A));
    assert!(!hooks.has_immediate_threat(&NimState { pile: 5, to_move: NimPlayer::A }, NimPlayer::A));
}
