fn kotost_skilltree_loot_drops(context: &horizon_client::mod_api::NativeLootDrops<'_>, drops: &mut Vec<horizon_client::items::ItemStack>) {
    crate::game::kotost_skilltree::loot_drops(context, drops);
}
