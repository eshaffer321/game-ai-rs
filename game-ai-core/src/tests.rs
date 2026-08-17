//! A toy game (subtract 1 or 2 from a shared pile; whoever takes the
//! last token wins) exercises the trait shape end to end without
//! depending on either real game adapter, which live in their own
//! repositories.

use crate::{Game, GameResult};

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

    fn evaluate(state: &Self::State) -> i32 {
        // Trivial: prefer leaving a multiple of 3 for the opponent.
        if state.pile % 3 == 0 {
            -1
        } else {
            1
        }
    }
}

#[test]
fn legal_moves_are_capped_by_the_remaining_pile() {
    let state = NimState { pile: 1, to_move: NimPlayer::A };
    assert_eq!(NimGame::legal_moves(&state), vec![1]);
}

#[test]
fn apply_move_flips_the_current_player() {
    let state = NimState { pile: 5, to_move: NimPlayer::A };
    let after = NimGame::apply_move(&state, 2);
    assert_eq!(after.pile, 3);
    assert_eq!(NimGame::current_player(&after), NimPlayer::B);
}

#[test]
fn taking_the_last_token_wins_for_the_player_who_just_moved() {
    let state = NimState { pile: 1, to_move: NimPlayer::A };
    let after = NimGame::apply_move(&state, 1);
    assert_eq!(NimGame::result(&after), GameResult::Win(NimPlayer::A));
    assert!(NimGame::result(&after).is_over());
}

#[test]
fn position_key_distinguishes_every_reachable_state() {
    let mut seen = std::collections::HashSet::new();
    for pile in 0..=6u8 {
        for &to_move in &[NimPlayer::A, NimPlayer::B] {
            let state = NimState { pile, to_move };
            assert!(seen.insert(NimGame::position_key(&state)), "duplicate key for {state:?}");
        }
    }
}

#[test]
fn other_player_is_its_own_inverse() {
    assert_eq!(NimGame::other_player(NimGame::other_player(NimPlayer::A)), NimPlayer::A);
    assert_eq!(NimGame::other_player(NimGame::other_player(NimPlayer::B)), NimPlayer::B);
}
