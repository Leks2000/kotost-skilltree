fn kotost_skilltree_scroll(game: &crate::game::Game<'_>, screen: &NativeScreenView<'_>, scroll: [f64; 2], canceled: &mut bool) {
    crate::game::kotost_skilltree::scroll(game, screen, scroll, canceled);
}
