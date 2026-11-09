# Third-party licenses

CcooNverter itself is released under the [MIT License](LICENSE). The release
binaries statically link the Rust crates below. Versions are those pinned in
`Cargo.lock` for v1.0.0; license expressions are the crates' own `license`
fields (checked with `cargo metadata`, 322 crates on the Windows target).

## GNU LGPL-2.1: rawler

| Crate | Version | License | Source |
|---|---|---|---|
| rawler | 0.8.0 | LGPL-2.1 | https://github.com/dnglab/dnglab (crate `rawler`) |

rawler (part of the dnglab project) decodes the RAW files. It is licensed under
the [GNU Lesser General Public License v2.1](https://github.com/dnglab/dnglab/blob/main/LICENSE).

rawler is statically linked into the CcooNverter executables and is used
unmodified. Its source is available from the link above and from crates.io. The
complete source of CcooNverter is published under the MIT license at
https://github.com/marushin/CcoonVerter together with `Cargo.lock`, so anyone
can modify rawler (or replace it with another version), point `Cargo.toml` at
their copy (for example with `[patch.crates-io] rawler = { path = "..." }`) and
rebuild with `cargo build --release -p ccoonverter`. Whether that arrangement
fully meets the LGPL-2.1 for a statically linked executable has not been
reviewed by a lawyer.

## Direct dependencies

| Crate | Version | License | Source |
|---|---|---|---|
| image | 0.25.10 | MIT OR Apache-2.0 | https://github.com/image-rs/image |
| jpeg-encoder | 0.7.1 | (MIT OR Apache-2.0) AND IJG | https://github.com/vstroebel/jpeg-encoder |
| png | 0.18.1 | MIT OR Apache-2.0 | https://github.com/image-rs/image-png |
| sysinfo | 0.37.2 | MIT | https://github.com/GuillaumeGomez/sysinfo |
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
| jpeg-encoder 0.7.1 | (MIT OR Apache-2.0) AND IJG | JPEG encoding, derived in part from libjpeg. Notice below |
| option-ext 0.2.0 | MPL-2.0 | used by `dirs-sys` (via `directories`); unmodified |
| ring 0.17.14 | Apache-2.0 AND ISC | TLS crypto for the update check (`rustls`) |
| libwebp-sys 0.14.4 | MIT, bundles libwebp (BSD-3-Clause, Google) | C library compiled into the binaries; lossy WebP encoding. Notice below |
| rustls 0.23.46 | Apache-2.0 OR ISC OR MIT | TLS for the update check |
| webpki-roots 1.0.11 | CDLA-Permissive-2.0 | Mozilla root certificates |
| clipboard-win, error-code | BSL-1.0 | Windows clipboard |
| self_cell 1.3.0 | Apache-2.0 OR GPL-2.0-only | used by `epaint`; used under Apache-2.0 |
| tiny-skia 0.11.4 | BSD-3-Clause | Linux only (Wayland window decorations) |
| alloc-no-stdlib, brotli-decompressor | BSD-3-Clause (OR MIT) | |
| moxcms, pxfm | BSD-3-Clause OR Apache-2.0 | colour handling, via `image`; used under Apache-2.0 |

The full, current list can be produced with
`cargo metadata --format-version 1` (field `license` of each package) or
`cargo tree -e normal --prefix none`. The license texts are included in each
crate's source on crates.io.

The Korean interface uses a system font (Malgun Gothic on Windows, Apple SD
Gothic Neo on macOS). No Korean font is included in the executable.

## Required notices

### libwebp (BSD-3-Clause)

```
Copyright (c) 2010, Google Inc. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

  * Redistributions of source code must retain the above copyright
    notice, this list of conditions and the following disclaimer.

  * Redistributions in binary form must reproduce the above copyright
    notice, this list of conditions and the following disclaimer in
    the documentation and/or other materials provided with the
    distribution.

  * Neither the name of Google nor the names of its contributors may
    be used to endorse or promote products derived from this software
    without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

libwebp also carries an additional patent grant from Google, in the file
`PATENTS` of its source (crate `libwebp-sys`, folder `vendor`).

### Independent JPEG Group

This software is based in part on the work of the Independent JPEG Group
(through the `jpeg-encoder` crate).

### Fonts

egui's built-in fonts carry their own licences: Noto Emoji (SIL Open Font
License 1.1), Ubuntu Light (Ubuntu Font Licence 1.0), Hack (Hack licence), and
the emoji icon font (MIT). The licence texts are in the `fonts` folder of the
`epaint_default_fonts` crate.
