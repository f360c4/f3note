# Portability targets

f3note commits to a **GTK 4.12 / GtkSourceView 5.10 API ceiling**. Those are the
newest versions available on the oldest distributions we promise native packages
for. Enabling a newer `v4_x`/`v5_x` feature in `Cargo.toml` silently drops the
targets below it — don't do it without updating this table.

| Distribution | GTK4 | GtkSourceView5 | Native package? |
|---|---|---|---|
| Arch / Fedora 43+ | 4.20+ | 5.18+ | yes |
| Debian 13 (trixie) | 4.18 | 5.16 | yes |
| Alpine 3.22 | 4.18 | 5.16 | yes |
| openSUSE Leap 16.0 | 4.18 | 5.16 | yes |
| RHEL / CentOS Stream 10 | 4.16 | 5.14 | yes |
| Ubuntu 24.04 LTS | 4.14 | 5.12 | yes |
| openSUSE Leap 15.6 | 4.12 | 5.10 | yes — sets the ceiling |
| Ubuntu 22.04 LTS | 4.6 | 5.4 | **AppImage only** |
| Debian 12 (bookworm) | 4.8 | 5.6 | **AppImage only** |
| RHEL / Rocky / Alma 9 | 4.12 | **none, not even EPEL** | **AppImage only** |

## Rust version

Distro rustc lags badly: Ubuntu 22.04/24.04 ship 1.91, Alpine 3.22 ships 1.87,
Debian 13 ships 1.85. Keep `rust-version` in `Cargo.toml` at or below the oldest
we care about, commit `Cargo.lock`, and verify with
`cargo +<oldest> check` in CI before tagging a release.

## Renderer

f3note sets `GSK_RENDERER=cairo` before initialising GTK, unless the user has
already set `GSK_RENDERER` or `F3NOTE_RENDERER`. This is not cosmetic: measured
exec-to-first-frame on an NVIDIA GTX 1650 / Hyprland 0.56.2 / GTK 4.22.4, with an
empty window holding one text view:

| Renderer | exec -> first frame |
|---|---|
| default (Vulkan) | 270-300 ms |
| `gl` | 250-260 ms |
| `cairo` | **130-140 ms** |

The 200 ms startup budget is not reachable with the default renderer on this
hardware. Vulkan context creation is likely cheaper on integrated GPUs — if you
have one, measure and report; the default may become conditional.
