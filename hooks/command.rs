fn kotost_skilltree_command(game: &mut crate::game::Game<'_>, player: &NativeServerPlayer, command: &str, ran: &mut bool) {
    crate::game::kotost_skilltree::perform_command(game, player, command, ran);
}
