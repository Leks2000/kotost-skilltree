fn kotost_skilltree_anvil(
    player: u128,
    left: &horizon_client::items::ItemStack,
    right: Option<&horizon_client::items::ItemStack>,
    output: &mut Option<horizon_client::items::ItemStack>,
    xp_cost: &mut i32,
    material_cost: &mut i32,
) {
    crate::game::kotost_skilltree::anvil_update(player, left, right, output, xp_cost, material_cost);
}
