-- archstaler: kind=e2e
-- Configuration for `cargo xtask e2e --config configs/e2e-aur.lua`: the e2e system plus one AUR package
-- (yay-bin, a prebuilt binary, so the build is quick). The second boot must build and install it.
local cfg = require("e2e").as

-- The AUR build needs a network on the installed system.
table.insert(cfg.packages.explicit, "networkmanager")
cfg.first_boot.services = { "NetworkManager.service", "ly@tty2.service" }

-- Pinned and reviewed with `cargo xtask aur-pin yay-bin`.
cfg.packages.aur = {
  { name = "yay-bin", pkgbase = "yay-bin", commit = "13e0a4754d106a9252b7479bf1b370fbe454fc48", sha256 = "b73aa937ff1cd4a76430393670c3316f71bf48ee2d996114db721faab227ee71", deps = { "base-devel", "git", "pacman" }, build_deps = { "base-devel" } },
}
-- No user archives or files here: this run is about the AUR step.
cfg.first_boot.user_archives = {}
cfg.first_boot.user_files = {}
return { as = cfg }
