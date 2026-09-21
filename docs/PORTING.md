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
| Ubuntu 22.04 LTS | 4.6 | 5.4 | **not supported** |
| Debian 12 (bookworm) | 4.8 | 5.6 | **not supported** |
| RHEL / Rocky / Alma 9 | 4.12 | **none, not even EPEL** | **not supported** |

## The three distributions that cannot run this

The bottom three in that table were meant to be covered by an AppImage. They
are not, and the reason is worth writing down so nobody re-plans around it.

An AppImage carries its libraries but not its C library: it runs on a system
whose glibc is at least as new as the one that built it. Covering Ubuntu 22.04
(glibc 2.35), Debian 12 (2.36) and RHEL 9 (2.34) means building on something
at least that old. But building f3note needs GTK 4.12, and none of those has
it — that is the same reason they cannot run a native build. The oldest
machine that can build this is Ubuntu 24.04, glibc 2.39, which is newer than
all three.

So the AppImage is a convenience for people who could also have compiled it.
It is not a route onto older systems, and the README says so.

What would actually work there is a **Flatpak**: the GNOME runtime supplies
GTK and its dependencies regardless of what the host has. That is the right
next step for these three, and it is not built yet.

## Rust version

`rust-version` is **1.92**, because twenty-one crates in the gtk-rs family
require it. That is higher than several distributions ship — Debian 13 has
1.85, Alpine 3.22 has 1.87, Ubuntu 24.04 has 1.91 — so building from source
on those needs `rustup` rather than the packaged toolchain.

This was declared as 1.85 for a while, which was simply untrue; CI now checks
the declared version instead of trusting it. Lowering it is not something this
project can decide on its own, and pretending otherwise only moves the failure
to whoever tries to package it.

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
