fn kotost_skilltree_net_from_server(game: &mut crate::game::Game<'_>, channel: &str, data: &[u8]) {
    crate::game::kotost_skilltree::net_from_server(game, channel, data);
}
