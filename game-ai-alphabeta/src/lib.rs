//! Iterative-deepening negamax generic over `G: game_ai_core::Game` --
//! a mechanical port of Onifish's original `onitama-ai::alphabeta`
//! engine, monomorphized per `(G, H: SearchHooks<G>)` pair (no `dyn`
//! in the hot path). See `../DESIGN.md`.

mod engine;
mod hooks;

pub use engine::{
    AlphaBetaAnalysis, AlphaBetaConfig, AlphaBetaPlayer, HistoryBonus, RootPolicyEvaluator, SearchLimit, MATE,
};
pub use hooks::{MoveFeatures, MovePriority, SearchHooks};

pub use game_ai_core::Game;
