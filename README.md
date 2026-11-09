[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![CI](https://github.com/marushin/CcoonVerter/actions/workflows/ci.yml/badge.svg)](https://github.com/marushin/CcoonVerter/actions/workflows/ci.yml)

# CcooNverter

**빠르고 간단한 RAW 배치 컨버터 (Windows / macOS).**
폴더를 고르고, 결과를 미리 보고, 버튼 하나로 하위 폴더까지 모든 RAW 파일을 JPG, PNG, 16비트 TIFF, WebP로 변환합니다.
설치 없이 실행되는 파일 하나로 배포됩니다. 사진 관리나 부분 보정은 하지 않습니다.

![CcooNverter 메인 화면](docs/screenshots/gui_windows_ko.png)

> English: [below](#english)

**처음이라면** [사용 안내](docs/GUIDE.ko.md)와 [자주 묻는 질문](docs/FAQ.ko.md)을 읽어 보세요.

## 다운로드

[Releases](https://github.com/marushin/CcoonVerter/releases/latest) 페이지에서 받으세요.

| OS | 파일 | 비고 |
|---|---|---|
| Windows 10/11 (x64) | `CcooNverter_v1.0.0_windows_x64.exe` | 설치 불필요. 그대로 실행 |
| macOS 11 이상 (Apple Silicon + Intel) | `CcooNverter_v1.0.0_macos_universal.dmg` | DMG를 열고 `CcooNverter.app`을 Applications로 드래그 |

`SHA256SUMS.txt`로 파일 무결성을 확인할 수 있습니다.

**처음 실행할 때 경고가 나옵니다.** 코드 서명이 없는 빌드이기 때문입니다. Windows에서는 "추가 정보 → 실행", macOS에서는 시스템 설정의 "그래도 열기"(또는 Control-클릭 → 열기)로 한 번 허용합니다. 자세한 방법과 이유는 [사용 안내](docs/GUIDE.ko.md#처음-실행할-때-나오는-경고)에 있습니다.

## 주요 기능

- **변환 전에 결과 보기**: 슬라이더를 움직이면 미리보기가 바로 바뀌고, 저장되는 결과와 같은 계산을 거칩니다. 100% 확대(실제 픽셀), 카메라 JPEG과 좌우 분할 비교, 히스토그램을 제공합니다.
- **폴더 훑어보기**: 미리보기 옆 썸네일 패널에서 파일을 눌러 바로 보고 화살표 키로 옮겨 다닙니다. 썸네일 크기와 패널 너비를 조절할 수 있고, 썸네일은 카메라 JPEG으로 만듭니다.
- **별점과 필터**: 마음에 드는 사진에 별을 주고(Bridge·Lightroom이 읽는 `.xmp` 사이드카에 저장, RAW는 그대로) 필터로 모아 봅니다. 필터가 켜지면 표시된 사진만 변환하며 실행 줄에 그 개수가 표시됩니다.
- **두 가지 방식**: RAW를 직접 현상하거나, 카메라가 RAW 안에 넣어 둔 JPEG을 재압축 없이 그대로 꺼냅니다.
- **현상**: 필름풍 기본 톤 커브(끄기 가능), 노출 -3~+3 EV, 카메라 JPEG 밝기에 맞춘 자동 노출, 클리핑된 하이라이트 색 깨짐 방지, 약한 출력 샤프닝.
- **스타일과 톤**: 지금 사진으로 그린 썸네일에서 고르는 12가지 픽처 스타일, 화이트 밸런스(카메라 / 자동), RGB 균형, 블랙/화이트 포인트.
- **출력 형식**: JPEG, PNG(8비트 무손실), 16비트 TIFF, WebP. sRGB 또는 AdobeRGB, ICC 프로파일 포함. 긴 변 크기와 품질 지정.
- **파일 이름 템플릿**: `{name}` `{date}` `{time}` `{seq}` `{camera}` `{ext}`.
- **메타데이터**: 카메라, 렌즈, 노출, 촬영 일시, GPS, 별점(XMP)을 파일에 있는 만큼 옮깁니다(기본으로 켜짐). 원본 파일 날짜 유지 옵션도 있습니다(기본으로 꺼짐).
- **일괄 작업**: 드래그 앤 드롭(폴더 또는 파일), 하위 폴더 구조 유지, 메모리에 맞춘 병렬 처리, 진행률과 남은 시간, 취소. 끝나면 앱 안의 결과 카드에 성공·실패 개수와 실패 이유를 표시합니다.
- **프리셋**: 설정을 이름으로 저장 (Python 버전의 `presets.json`도 읽습니다).
- **한국어 / 영어 UI**: 시스템 언어 자동 감지, 모든 설정에 툴팁.
- **RAW 원본은 수정하지 않습니다.**

자세한 변경 내역은 [CHANGELOG.md](CHANGELOG.md)를 보세요.

| 비교 | 결과 카드 |
|---|---|
| ![비교](docs/screenshots/split_ko.png) | ![결과 카드](docs/screenshots/result_ko.png) |

## 지원 포맷

| 제조사 | 확장자 | 실제 파일로 확인한 기종 | 알려진 제한 |
|---|---|---|---|
| Sony | `.ARW` | ILCE-7S, DSLR-A200, ILCE-1M2 (무손실) | ILCE-7M5(A7 V)는 현상 불가, 카메라 JPEG 추출 가능 |
| Canon | `.CR2` | EOS 20D, EOS 40D sRAW, PowerShot G12 | |
| Canon | `.CR3` | EOS R6, EOS R5 Mark II, EOS 250D (C-RAW) | |
| Nikon | `.NEF` | D70, Z 6, Z 30 (무손실) | HE/HE\*(Z6III, Z f 등)는 현상 불가, 카메라 JPEG 추출 가능 |
| Adobe / 기타 | `.DNG` | EOS 5D Mark III DNG, Adobe DNG(손실 압축), iPhone XS | |
| Fujifilm | `.RAF` | X-S10, X-T4 (X-Trans), X-A1 (베이어), FinePix S5000 | X-Trans는 단순(바이리니어) 디모자이크 |
| Olympus / OM System | `.ORF` | E-PL2, TG-6, OM-5 | |
| Panasonic | `.RW2` | DC-G100, DMC-G2, DMC-GH4 | |
| Pentax | `.PEF` | K-5 II s, K-x | |
| Samsung | `.SRW` | NX500, NX3000, EX1 | |

확장자는 대소문자를 구분하지 않습니다. RAW 디코딩은 [rawler](https://github.com/dnglab/dnglab) 0.8.0을 사용합니다. 위 표는 raw.pixls.us CC0 샘플 34개로 만든 회귀 매트릭스 결과이며, 실행 방법은 [tests/regression/README.md](tests/regression/README.md)에 있습니다.

## 출력 형식과 메타데이터

| 형식 | 비트 | ICC | EXIF | 별점 |
|---|---|---|---|---|
| JPEG | 8 | 포함 | 포함 | Exif 태그, XMP |
| PNG | 8 | 포함 | 포함 | Exif 태그, XMP |
| WebP | 8 (손실) | 포함 | 포함 | Exif 태그, XMP |
| TIFF | 16 | 포함 | 메이커노트를 뺀 EXIF/GPS | Exif 태그, XMP |

카메라 JPEG 추출은 항상 JPEG이고, 카메라 파일을 그대로 쓰므로 ICC를 새로 넣지 않습니다. 알아 둘 한계(AdobeRGB, 메이커노트, 지원하지 않는 카메라, 손상 파일)는 [FAQ](docs/FAQ.ko.md)에 있습니다.

## 설정과 로그 위치

| OS | 설정·프리셋 | 로그 | 업데이트 캐시 |
|---|---|---|---|
| Windows | `%APPDATA%\ccoon\CcooNverter\config\` | `%LOCALAPPDATA%\ccoon\CcooNverter\data\logs\ccoonverter.log` | `%LOCALAPPDATA%\ccoon\CcooNverter\cache\` |
| macOS | `~/Library/Application Support/ccoon.CcooNverter/` | `~/Library/Application Support/ccoon.CcooNverter/logs/ccoonverter.log` | `~/Library/Caches/ccoon.CcooNverter/` |
| Linux | `~/.config/ccoonverter/` | `~/.local/share/ccoonverter/logs/ccoonverter.log` | `~/.cache/ccoonverter/` |

- 설정 파일은 `settings.json`, 프리셋은 `presets.json`입니다. 로그는 5MB를 넘으면 `ccoonverter.log.1`로 교체됩니다.
- **예전 설정 가져오기**: Python 버전(0.8.x) 실행 파일과 **같은 폴더**에 새 실행 파일을 두고 처음 실행하면 `converter_settings.json`과 `presets.json`을 한 번 가져옵니다(새 설정 파일이 아직 없을 때만).
- **포터블 모드**: 환경 변수 `CCOONVERTER_HOME`을 지정하면 모든 파일을 그 폴더 아래(`config/`, `data/logs/`, `cache/`)에 저장합니다.
  ```powershell
  $env:CCOONVERTER_HOME = "D:\CcooNverter-portable"; .\CcooNverter_v1.0.0_windows_x64.exe
  ```

## 명령줄 도구 (`ccoonverter-cli`)

GUI와 같은 변환 엔진을 쓰는 명령줄 도구가 있습니다(소스에서 빌드).

```text
ccoonverter-cli convert <SOURCE> [-o DIR] [--resize N] [-q Q] [--brightness F] [--wb camera|auto]
                        [--ev EV] [--no-tone-curve] [--sharpen F] [--style NAME] [--rgb R,G,B] [--levels BP,WP] [--embedded] [--exif] [--keep-date]
                        [--on-conflict skip|overwrite|suffix] [--format jpeg|png|tiff|webp] [--color-space srgb|adobergb]
                        [--name-template TEMPLATE] [--preset NAME] [-j N] [--json]
ccoonverter-cli scan <DIR>         # 변환 대상 RAW 파일 목록
ccoonverter-cli info <RAW_FILE>    # 디코더, 해상도, 임베디드 JPEG, 주요 EXIF
```

종료 코드: 0 성공, 1 일부 파일 실패, 2 사용법 오류. `ccoonverter-cli convert --help`에 전체 옵션이 있습니다.

## 소스에서 빌드

[Rust](https://rustup.rs) stable이 필요합니다.

```bash
# GUI (target/release/CcooNverter[.exe])
cargo build --release -p ccoonverter
# CLI
cargo build --release -p ccoonverter-cli
# 테스트
cargo test --workspace
```

- **Windows 배포 파일**: `powershell -ExecutionPolicy Bypass -File scripts\build_windows.ps1` → `dist\CcooNverter_v<버전>_windows_x64.exe`, `dist\SHA256SUMS.txt`
- **macOS 앱/DMG**: `xcode-select --install` 후 `bash scripts/build_macos.sh` → `dist/macos/CcooNverter.app`, `dist/macos/CcooNverter_v<버전>_macos_universal.dmg` (universal, ad-hoc 서명). 확인 항목은 [docs/03_MAC_BUILD_PLAN.md](docs/03_MAC_BUILD_PLAN.md) §7에 있습니다.
- **렌더러**: 기본 빌드는 OpenGL(glow)로 그립니다. OpenGL 드라이버에 문제가 있으면 wgpu(DirectX 12 / Vulkan / Metal) 렌더러로 빌드할 수 있습니다: `cargo build --release -p ccoonverter --features wgpu`
- **Linux 빌드**에는 `libgtk-3-dev libxkbcommon-dev libwayland-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libgl1-mesa-dev`가 필요합니다(공식 배포는 Windows/macOS만).

## 릴리스 절차 (관리자용)

1. `Cargo.toml`의 `[workspace.package] version`과 `CHANGELOG.md`의 `## vX.Y.Z` 절을 맞춥니다.
2. `main`에 푸시하면 [CI](https://github.com/marushin/CcoonVerter/actions/workflows/ci.yml)가 Linux/Windows/macOS에서 fmt, clippy, 테스트, 릴리스 빌드를 실행합니다.
3. `git tag vX.Y.Z && git push origin vX.Y.Z`를 하면 [Release 워크플로](.github/workflows/release.yml)가 Windows exe, macOS universal DMG, `SHA256SUMS.txt`를 만들어 GitHub Release 드래프트에 첨부합니다. 본문은 CHANGELOG의 해당 절입니다. 받아서 Windows/macOS에서 확인한 뒤 GitHub에서 **Publish release**를 누릅니다.
4. 코드 서명과 공증은 저장소 시크릿을 등록했을 때만 실행됩니다([docs/SIGNING.md](docs/SIGNING.md)).

## 라이선스

- CcooNverter: [MIT](LICENSE)
- RAW 디코딩에 쓰는 **rawler**는 LGPL-2.1, WebP 인코딩에 쓰는 **libwebp**는 BSD-3-Clause입니다. 그 밖의 서드파티 고지는 [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)를 보세요.

## Python 버전

v0.8.7 이하는 Python(CustomTkinter + rawpy) 앱이었습니다. 예전 실행 파일은 [Releases](https://github.com/marushin/CcoonVerter/releases)의 v0.8.x 항목에서 받을 수 있습니다. 자세한 안내: [legacy/README.md](legacy/README.md).

---

## English

**CcooNverter** is a fast batch RAW converter for Windows and macOS. Pick a folder, check the result in the preview, press one button, and every RAW file (sub-folders included) becomes a JPEG, PNG, 16-bit TIFF or WebP. It ships as a single portable executable on Windows and a universal app on macOS. It does not manage photos or do local edits.

![CcooNverter main window](docs/screenshots/gui_windows.png)

**New here?** Read the [user guide](docs/GUIDE.en.md) and the [FAQ](docs/FAQ.en.md).

### Download

[Releases](https://github.com/marushin/CcoonVerter/releases/latest): `CcooNverter_v1.0.0_windows_x64.exe` or `CcooNverter_v1.0.0_macos_universal.dmg`, with `SHA256SUMS.txt`.

**Expect a warning on first launch**, because these builds are not code signed. On Windows choose "More info → Run anyway". On macOS use "Open Anyway" in System Settings (or Control-click → Open on older versions). The [guide](docs/GUIDE.en.md#the-warning-on-first-launch) explains why and how.

### Features

- **See the result before converting**: the preview updates as you move a slider and goes through the same processing as the saved file. 100 % view with real pixels, side-by-side split against the camera JPEG, histogram.
- **Browse the folder**: click a thumbnail in the panel beside the preview, or move with the arrow keys. Thumbnail size and panel width are adjustable, and thumbnails come from the camera JPEG.
- **Stars and a filter**: give photos you like some stars (saved in an `.xmp` sidecar that Bridge and Lightroom read; the RAW stays as it is) and gather them with the filter. With the filter on, only the photos shown are converted, and the run bar says how many.
- **Two modes**: develop the RAW, or take out the JPEG the camera saved inside it without compressing it again.
- **Development**: default film-like tone curve (can be turned off), exposure from -3 to +3 EV, automatic exposure anchored on the camera JPEG, clean clipped highlights, light output sharpening.
- **Styles and tone**: 12 picture styles chosen from thumbnails of your own photo, white balance (camera or auto), RGB balance, black and white points.
- **Output**: JPEG, PNG (8-bit lossless), 16-bit TIFF or lossy WebP, sRGB or AdobeRGB with an ICC profile, long edge and quality.
- **File name templates**: `{name}` `{date}` `{time}` `{seq}` `{camera}` `{ext}`.
- **Metadata**: optional copy of camera, lens, exposure, date, GPS and rating (XMP), and of the source file date. Off by default.
- **Batches**: drag and drop of folders or files, sub-folder structure kept, parallel processing sized to your memory, progress, time left and cancel. A result card in the window shows counts and why files failed.
- **Presets**, also reading presets from the Python version.
- **Korean and English interface**, auto-detected, with a tooltip on every setting.
- **Your RAW files are never modified.**

### Supported formats

| Maker | Extension | Verified on real files | Known limits |
|---|---|---|---|
| Sony | `.ARW` | ILCE-7S, DSLR-A200, ILCE-1M2 (lossless) | ILCE-7M5 (A7 V) cannot be developed; camera JPEG extraction works |
| Canon | `.CR2` | EOS 20D, EOS 40D sRAW, PowerShot G12 | |
| Canon | `.CR3` | EOS R6, EOS R5 Mark II, EOS 250D (C-RAW) | |
| Nikon | `.NEF` | D70, Z 6, Z 30 (lossless) | HE/HE\* (Z6III, Z f and others) cannot be developed; camera JPEG extraction works |
| Adobe / others | `.DNG` | EOS 5D Mark III DNG, Adobe DNG (lossy), iPhone XS | |
| Fujifilm | `.RAF` | X-S10, X-T4 (X-Trans), X-A1 (Bayer), FinePix S5000 | X-Trans uses simple (bilinear) demosaicing |
| Olympus / OM System | `.ORF` | E-PL2, TG-6, OM-5 | |
| Panasonic | `.RW2` | DC-G100, DMC-G2, DMC-GH4 | |
| Pentax | `.PEF` | K-5 II s, K-x | |
| Samsung | `.SRW` | NX500, NX3000, EX1 | |

Extensions are not case sensitive. RAW decoding uses [rawler](https://github.com/dnglab/dnglab) 0.8.0. The table comes from a regression matrix of 34 CC0 samples from raw.pixls.us; see [tests/regression/README.md](tests/regression/README.md). The limits worth knowing (AdobeRGB, MakerNote, unsupported cameras, damaged files) are in the [FAQ](docs/FAQ.en.md).

### Settings, command line, building

- **Settings** live in the OS standard folders (table above); settings from the Python version next to the executable are imported once; set `CCOONVERTER_HOME` for a portable setup.
- **CLI**: `ccoonverter-cli convert|scan|info` shares the same engine (build from source; see `convert --help`).
- **Build**: `cargo build --release -p ccoonverter` (OpenGL/glow renderer; add `--features wgpu` for wgpu), `scripts/build_windows.ps1`, `scripts/build_macos.sh`.

### License

MIT. rawler is LGPL-2.1 and libwebp is BSD-3-Clause; see [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md). The Python version (≤ v0.8.7) remains available from the v0.8.x releases.
