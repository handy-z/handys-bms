use handy_sdk::roblox::offsets::{OffsetKeys, SDK_KEYS};
use handy_sdk::roblox::read::join_keys;
use handy_sdk::roblox::screen::SCREEN_KEYS;

/// Every offset the app reads; the Updater rejects a dump missing any. Starts as SDK_KEYS: add each key a reader in
/// src/game/ uses.
pub fn required_keys() -> OffsetKeys {
    join_keys(
        &[&SDK_KEYS, &SCREEN_KEYS], // overlay/engine
        &[
            "DataModel.Workspace",
            "BasePart.Primitive",
            "BasePart.Color3",
            "Primitive.Position",
            "GuiObject.Text",
            "Player.LocalPlayer",
        ], // game/board
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_keys_cover_sdk_keys() {
        let keys = required_keys();
        for k in SDK_KEYS {
            assert!(keys.contains(&k), "{k}");
        }
    }
}
