# Third-party licenses

CcooNverter itself is released under the [MIT License](LICENSE). The release
binaries statically link the Rust crates below. Versions are those pinned in
`Cargo.lock` for v0.9.0; license expressions are the crates' own `license`
fields (checked with `cargo metadata`).

## GNU LGPL-2.1: rawler

| Crate | Version | License | Source |
|---|---|---|---|
| rawler | 0.8.0 | LGPL-2.1 | https://github.com/dnglab/dnglab (crate `rawler`) |

rawler (part of the dnglab project) decodes the RAW files. It is licensed under
the [GNU Lesser General Public License v2.1](https://github.com/dnglab/dnglab/blob/main/LICENSE).

**LGPL-2.1 §6 notice.** rawler is statically linked into the CcooNverter
executables. The complete source code of CcooNverter is published under the MIT
license at https://github.com/marushin/CcoonVerter together with `Cargo.lock`,
and rawler's source is available from the link above and from crates.io. You may
modify rawler (or replace it with another version), point `Cargo.toml` at your
copy (for example with `[patch.crates-io] rawler = { path = "..." }`) and rebuild
the application with `cargo build --release -p ccoonverter`. This satisfies the
relinking requirement of LGPL-2.1 §6(a). rawler is used unmodified.

## Direct dependencies

| Crate | Version | License | Source |
|---|---|---|---|
| image | 0.25.10 | MIT OR Apache-2.0 | https://github.com/image-rs/image |
| jpeg-encoder | 0.7.1 | (MIT OR Apache-2.0) AND IJG | https://github.com/vstroebel/jpeg-encoder |
| img-parts | 0.4.2 | MIT OR Apache-2.0 | https://github.com/paolobarbolini/img-parts |
| kamadak-exif | 0.6.2 | BSD-2-Clause | https://github.com/kamadak/exif-rs |
| rayon | 1.12.0 | MIT OR Apache-2.0 | https://github.com/rayon-rs/rayon |
| serde | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_json | 1.0.154 | MIT OR Apache-2.0 | https://github.com/serde-rs/json |
| ureq | 3.5.1 | MIT OR Apache-2.0 | https://github.com/algesten/ureq |
| directories | 6.0.0 | MIT OR Apache-2.0 | https://github.com/soc/directories-rs |
| semver | 1.0.28 | MIT OR Apache-2.0 | https://github.com/dtolnay/semver |
| log | 0.4.35 | MIT OR Apache-2.0 | https://github.com/rust-lang/log |
| fern | 0.7.1 | MIT | https://github.com/daboross/fern |
| chrono | 0.4.45 | MIT OR Apache-2.0 | https://github.com/chronotope/chrono |
| thiserror | 2.0.21 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| anyhow | 1.0.104 | MIT OR Apache-2.0 | https://github.com/dtolnay/anyhow |
| clap | 4.6.7 | MIT OR Apache-2.0 | https://github.com/clap-rs/clap |
| eframe | 0.37.0 | MIT OR Apache-2.0 | https://github.com/emilk/egui |
| egui / epaint / egui-winit / egui_glow | 0.37.0 | MIT OR Apache-2.0 | https://github.com/emilk/egui |
| winit | 0.30.13 | Apache-2.0 | https://github.com/rust-windowing/winit |
| glow | 0.17.0 | MIT OR Apache-2.0 OR Zlib | https://github.com/grovesNL/glow |
| glutin | 0.32.3 | Apache-2.0 | https://github.com/rust-windowing/glutin |
| raw-window-handle | 0.6.2 | MIT OR Apache-2.0 OR Zlib | https://github.com/rust-windowing/raw-window-handle |
| accesskit | 0.25.2 | MIT OR Apache-2.0 | https://github.com/AccessKit/accesskit |
| rfd | 0.18.0 | MIT | https://github.com/PolyMeilex/rfd |
| open | 5.4.5 | MIT | https://github.com/Byron/open-rs |
| windows-sys (Windows only) | 0.61.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| winresource (build tool, Windows) | 0.1.34 | MIT | https://github.com/BenjaminRi/winresource |
| wgpu (only with `--features wgpu`) | 30.0.1 | MIT OR Apache-2.0 | https://github.com/gfx-rs/wgpu |

## Notable transitive dependencies

Almost all other crates are `MIT OR Apache-2.0`, `MIT`, `Apache-2.0`, `Zlib`,
`BSD-2/3-Clause`, `ISC`, `Unicode-3.0` or `Unlicense OR MIT`. Crates with other
or additional terms:

| Crate | License | Note |
|---|---|---|
| epaint_default_fonts 0.37.0 | (MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0 | egui's built-in fonts (Ubuntu Light, Noto Emoji, Hack) |
| option-ext 0.2.0 | MPL-2.0 | used by `dirs-sys` (via `directories`); unmodified |
| ring 0.17.14 | Apache-2.0 AND ISC | TLS crypto for the update check (`rustls`) |
| rustls 0.23.46 | Apache-2.0 OR ISC OR MIT | TLS for the update check |
| webpki-roots 1.0.11 | CDLA-Permissive-2.0 | Mozilla root certificates |
| clipboard-win, error-code | BSL-1.0 | Windows clipboard |
| self_cell 1.3.0 | Apache-2.0 OR GPL-2.0-only | used by `epaint`; used under Apache-2.0 |
| tiny-skia 0.11.4 | BSD-3-Clause | Linux only (Wayland window decorations) |
| alloc-no-stdlib, brotli-decompressor | BSD-3-Clause (OR MIT) | |

The full, current list can be produced with
`cargo metadata --format-version 1` (field `license` of each package) or
`cargo tree -e normal --prefix none`. The license texts are included in each
crate's source on crates.io.
