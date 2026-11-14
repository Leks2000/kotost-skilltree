fn kotost_skilltree_server_tick(game: &mut crate::game::Game<'_>, phase: horizon_client::mod_api::ServerTickPhase) {
    crate::game::kotost_skilltree::server_tick_hook(game, phase);
}
