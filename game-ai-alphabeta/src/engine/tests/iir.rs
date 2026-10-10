use super::*;

struct IirHooks {
    allowed: bool,
    threat: Option<NimPlayer>,
}

impl SearchHooks<AcyclicNimGame> for IirHooks {
    const HISTORY_BUCKETS: usize = 3;
    type EvalState = ();

    fn init_eval_state(&self, _state: &NimState) {}
    fn evaluate(&self, state: &NimState, _: &()) -> i32 {
        <NimHooks as SearchHooks<NimGame>>::evaluate(&NimHooks, state, &())
    }
    fn move_features(&self, state: &NimState, mv: &u8) -> MoveFeatures {
        <NimHooks as SearchHooks<NimGame>>::move_features(&NimHooks, state, mv)
    }
    fn has_immediate_threat(&self, _state: &NimState, player: NimPlayer) -> bool {
        self.threat == Some(player)
    }
    fn supports_internal_iterative_reduction(&self, _state: &NimState) -> bool {
        self.allowed
    }
}

fn context() -> SearchContext<AcyclicNimGame> {
    let max_ply = 32;
    SearchContext {
        tt: TranspositionTable::new(1),
        killers: vec![[None; 2]; max_ply + 1],
        history: vec![0; 3],
        nodes: 0,
        quiescence_nodes: 0,
        tt_hits: 0,
        beta_cutoffs: 0,
        tt_cutoffs: 0,
        rfp_attempts: 0,
        rfp_cutoffs: 0,
        iir_reductions: 0,
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
        authoritative_tt: true,
        rfp: None,
        iir: true,
        eval_history: Vec::new(),
        root_policy: None,
        node_limit: None,
        deadline: None,
        aborted: false,
        path: Vec::new(),
        max_ply,
        use_tt: true,
        move_buffers: vec![Vec::new(); max_ply + 1],
    }
}

#[test]
fn defaults_do_not_enable_iir() {
    assert!(!AlphaBetaConfig::default().iir);
    let state = NimState {
        pile: 30,
        to_move: NimPlayer::A,
    };
    assert!(!<NimHooks as SearchHooks<AcyclicNimGame>>::supports_internal_iterative_reduction(
        &NimHooks, &state
    ));
}

#[test]
fn eligibility_checks_root_depth_tt_phase_and_both_players_threats() {
    let state = NimState {
        pile: 30,
        to_move: NimPlayer::A,
    };
    let hooks = IirHooks {
        allowed: true,
        threat: None,
    };
    assert!(iir_eligible(&hooks, &state, 1, 4, false));
    assert!(!iir_eligible(&hooks, &state, 0, 4, false));
    assert!(!iir_eligible(&hooks, &state, 1, 3, false));
    assert!(!iir_eligible(&hooks, &state, 1, 4, true));
    assert!(!iir_eligible(
        &IirHooks {
            allowed: false,
            threat: None
        },
        &state,
        1,
        4,
        false
    ));
    for threat in [NimPlayer::A, NimPlayer::B] {
        assert!(!iir_eligible(
            &IirHooks {
                allowed: true,
                threat: Some(threat)
            },
            &state,
            1,
            4,
            false
        ));
    }
}

#[test]
fn tt_stores_the_reduced_depth_and_cannot_serve_it_as_a_deeper_exact_result() {
    let state = NimState {
        pile: 30,
        to_move: NimPlayer::A,
    };
    let hooks = IirHooks {
        allowed: true,
        threat: None,
    };
    let mut ctx = context();
    assert!(negamax(&hooks, &mut ctx, &state, &(), 4, -MATE - 1, MATE + 1, 1).is_some());
    assert_eq!(ctx.iir_reductions, 1);
    assert_eq!(ctx.tt.probe(AcyclicNimGame::position_key(&state)).unwrap().depth, 3);

    let before = ctx.nodes;
    let reductions = ctx.iir_reductions;
    assert!(negamax(&hooks, &mut ctx, &state, &(), 4, -MATE - 1, MATE + 1, 1).is_some());
    assert!(ctx.nodes - before > 1, "a depth-3 entry must not cut off a depth-4 request");
    assert_eq!(ctx.iir_reductions, reductions, "a shallow TT hit suppresses IIR too");
    assert_eq!(ctx.tt.probe(AcyclicNimGame::position_key(&state)).unwrap().depth, 4);
}

#[test]
fn disabling_the_table_is_a_real_miss_and_still_respects_the_node_budget() {
    let state = NimState {
        pile: 30,
        to_move: NimPlayer::A,
    };
    let hooks = IirHooks {
        allowed: true,
        threat: None,
    };
    let mut ctx = context();
    ctx.use_tt = false;
    ctx.node_limit = Some(20);
    let result = negamax(&hooks, &mut ctx, &state, &(), 9, -MATE - 1, MATE + 1, 0);
    assert!(result.is_none());
    assert!(ctx.aborted);
    assert_eq!(ctx.nodes, 20);
    assert!(ctx.iir_reductions > 0);
    assert!(ctx.path.is_empty(), "aborting must unwind the search path");
}

#[test]
fn hooks_that_do_not_opt_in_keep_identical_search_results() {
    let state = NimState {
        pile: 20,
        to_move: NimPlayer::A,
    };
    let config = AlphaBetaConfig {
        limit: SearchLimit::Depth(7),
        ..AlphaBetaConfig::default()
    };
    let mut baseline = AlphaBetaPlayer::<AcyclicNimGame, _>::new(config.clone(), NimHooks);
    let mut candidate = AlphaBetaPlayer::<AcyclicNimGame, _>::new(AlphaBetaConfig { iir: true, ..config }, NimHooks);
    let a = baseline.analyze(&state, None);
    let b = candidate.analyze(&state, None);
    assert_eq!(b.iir_reductions, 0);
    assert_eq!(
        (a.best_move, a.score, a.nodes, a.completed_depth, a.principal_variation),
        (b.best_move, b.score, b.nodes, b.completed_depth, b.principal_variation)
    );
}
