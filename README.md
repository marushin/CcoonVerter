[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![CI](https://github.com/marushin/CcoonVerter/actions/workflows/ci.yml/badge.svg)](https://github.com/marushin/CcoonVerter/actions/workflows/ci.yml)

# CcooNverter

**빠르고 간단한 RAW → JPG 배치 컨버터 (Windows / macOS).**
폴더를 고르고 버튼 하나로 하위 폴더까지 모든 RAW 파일을 JPG로 변환합니다.
v0.9.0부터 Rust로 전면 재작성되어 설치 없이 실행되는 단일 실행 파일로 배포됩니다.

![CcooNverter Windows 스크린샷](docs/screenshots/gui_windows.png)

> English summary: [below](#english).

## 다운로드

[Releases](https://github.com/marushin/CcoonVerter/releases/latest) 페이지에서 받으세요.

| OS | 파일 | 비고 |
|---|---|---|
| Windows 10/11 (x64) | `CcooNverter_v0.10.0_windows_x64.exe` | 설치 불필요. 그대로 실행 |
| macOS 11 이상 (Apple Silicon + Intel) | `CcooNverter_v0.10.0_macos_universal.dmg` | DMG를 열고 `CcooNverter.app`을 Applications로 드래그 |

`SHA256SUMS.txt`로 파일 무결성을 확인할 수 있습니다.

### macOS 첫 실행

앱이 Apple 공증(notarization)을 받지 않았기 때문에 처음 실행할 때 "확인되지 않은 개발자" 경고가 뜹니다. 다음 중 하나로 한 번만 허용하면 됩니다.

- Finder에서 `CcooNverter.app`을 **우클릭(Control-클릭) → 열기 → 열기**
- 또는 터미널에서:
  ```bash
  xattr -dr com.apple.quarantine /Applications/CcooNverter.app
  ```

## 주요 기능

- **미리보기 패널**(v0.10.0): 변환 전에 결과를 확인. 넓은 창에서는 설정 옆, 좁은 창에서는 SHOW PREVIEW로 펼침. 슬라이더를 움직이면 바로 다시 그리고, 카메라 임베디드 JPEG과 before/after 비교 가능
- **드래그 앤 드롭**(v0.10.0): 폴더를 놓으면 원본 폴더로 지정, RAW 파일을 놓으면 그 파일들만 변환 대상으로 선택
- **현상 품질**(v0.10.0): 기본 필름 톤 커브(끄기 가능), Exposure(EV) -3~+3, 클리핑된 하이라이트 색 깨짐 방지, 약한 출력 샤프닝
- **12가지 픽처 스타일**: Standard, Portrait, Landscape, Neutral, Vivid, Monochrome, Sepia, Cinematic, HDR, Soft, Dramatic, Black & White
- **세밀한 톤 컨트롤**: 밝기, RGB 채널 밸런스, Black/White Point(Levels), 화이트 밸런스(카메라 / 자동)
- **RAW 현상**: Sony, Canon, Nikon, Adobe DNG 주요 포맷 (아래 표)
- **임베디드 JPEG 추출(빠름)**: 카메라가 RAW 안에 넣어 둔 JPEG을 꺼내 저장. 리사이즈가 없으면 **재압축 없이 원본 그대로 복사**(무손실)
- **리사이즈(긴 변 px)와 JPG 품질(1~100)**
- **EXIF 보존**: 촬영 정보(카메라, 렌즈, 노출, 촬영 일시 등)를 결과 JPG에 기록
- **프리셋**: 설정을 이름으로 저장하고 다시 불러오기 (Python 버전의 `presets.json`과 호환)
- **멀티코어 병렬 변환**, 진행률과 남은 시간(ETA), 변환 취소
- **하위 폴더 구조 유지**: 기본 출력 폴더는 `<원본 폴더>/Converted_JPG`

v0.9.0에서 새로 생기거나 바뀐 점:

- Windows와 **macOS(universal)** 동시 지원
- 출력 파일이 이미 있으면 **덮어쓰지 않고 접미사**(`_arw`, `_1` …)를 붙임. 건너뛰기/덮어쓰기도 선택 가능
- 사진 방향(세로 사진)을 픽셀에 반영해 어떤 뷰어에서도 똑바로 보임
- 변환 완료 후 **출력 폴더 열기**
- 업데이트 확인 **켜기/끄기** 옵션(24시간 캐시)
- 마지막으로 쓴 원본 폴더 기억
- 설정·프리셋·로그를 OS 표준 위치에 저장하고, 실행 파일 옆의 예전 설정은 첫 실행 때 자동으로 가져옴

자세한 변경 내역은 [CHANGELOG.md](CHANGELOG.md)를 보세요.

## 지원 포맷

| 제조사 | 확장자 | 실제 파일 검증 (v0.9.0) |
|---|---|---|
| Sony | `.ARW` | ILCE-7S |
| Canon | `.CR2` | EOS 7D Mark II |
| Canon | `.CR3` | EOS R6 |
| Nikon | `.NEF` | D750 (Nikon Z HE/HE* 압축은 미검증) |
| Adobe / 기타 | `.DNG` | EOS 5D Mark III DNG |

확장자는 대소문자를 구분하지 않습니다. RAW 디코딩은 순수 Rust 라이브러리 [rawler](https://github.com/dnglab/dnglab)를 사용합니다. 검증 상세는 [docs/SAMPLE_VERIFICATION.md](docs/SAMPLE_VERIFICATION.md).

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
  $env:CCOONVERTER_HOME = "D:\CcooNverter-portable"; .\CcooNverter_v0.10.0_windows_x64.exe
  ```

## 명령줄 도구 (`ccoonverter-cli`)

GUI와 같은 변환 엔진을 쓰는 명령줄 도구가 있습니다(소스에서 빌드).

```text
ccoonverter-cli convert <SOURCE> [-o DIR] [--resize N] [-q Q] [--brightness F] [--wb camera|auto]
                        [--ev EV] [--no-tone-curve] [--sharpen F] [--style NAME] [--rgb R,G,B] [--levels BP,WP] [--embedded] [--exif]
                        [--on-conflict skip|overwrite|suffix] [--preset NAME] [-j N] [--json]
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
- **macOS 앱/DMG**: `xcode-select --install` 후 `bash scripts/build_macos.sh` → `dist/macos/CcooNverter.app`, `dist/macos/CcooNverter_v<버전>_macos_universal.dmg` (universal, ad-hoc 서명)
- **렌더러**: 기본 빌드는 OpenGL(glow)로 그립니다(작은 실행 파일, 대부분의 PC·원격 데스크톱에서 동작). OpenGL 드라이버에 문제가 있으면 wgpu(DirectX 12 / Vulkan / Metal) 렌더러로 빌드할 수 있습니다:
  ```bash
  cargo build --release -p ccoonverter --features wgpu
  ```
- Linux 빌드에는 `libgtk-3-dev libxkbcommon-dev libwayland-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libgl1-mesa-dev`가 필요합니다(공식 배포는 Windows/macOS만).

### macOS에서 직접 빌드하고 확인하기

```bash
xcode-select --install                      # 처음 한 번
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust 설치(처음 한 번)
git clone https://github.com/marushin/CcoonVerter.git && cd CcoonVerter
bash scripts/build_macos.sh                 # arm64 + x86_64 빌드 → .app → ad-hoc 서명 → DMG
open dist/macos/CcooNverter_v0.10.0_macos_universal.dmg
```

확인 항목: `file dist/macos/CcooNverter.app/Contents/MacOS/CcooNverter`가 "universal binary with 2 architectures"로 나오는지, 우클릭 → 열기로 창이 뜨는지, 한글 폴더를 골라 변환이 되는지, 설정이 `~/Library/Application Support/ccoon.CcooNverter/settings.json`에 저장되는지. 전체 체크리스트는 [docs/03_MAC_BUILD_PLAN.md](docs/03_MAC_BUILD_PLAN.md) §7에 있습니다. 창이 뜨지 않거나 그리기가 깨지면 `cargo build --release -p ccoonverter --features wgpu`(Metal)로 다시 빌드해 보세요.

## 릴리스 절차 (관리자용)

1. `Cargo.toml`의 `[workspace.package] version`과 `CHANGELOG.md`의 `## vX.Y.Z` 절을 맞춥니다.
2. `main`에 푸시하면 [CI](https://github.com/marushin/CcoonVerter/actions/workflows/ci.yml)가 Linux/Windows/macOS에서 fmt, clippy, 테스트, 릴리스 빌드를 실행합니다.
3. `git tag vX.Y.Z && git push origin vX.Y.Z`를 하면 [Release 워크플로](.github/workflows/release.yml)가 Windows exe, macOS universal DMG, `SHA256SUMS.txt`를 만들어 GitHub Release에 첨부합니다. 릴리스 본문은 CHANGELOG의 해당 절입니다. 릴리스는 **드래프트**로 만들어지므로, 첨부된 파일을 받아 Windows/macOS에서 확인한 뒤 GitHub에서 **Publish release**를 눌러 공개합니다.

## 라이선스

- CcooNverter: [MIT](LICENSE)
- RAW 디코딩에 쓰는 **rawler**는 [LGPL-2.1](https://github.com/dnglab/dnglab/blob/main/LICENSE) 라이선스입니다. CcooNverter는 전체 소스를 공개하므로 사용자가 rawler를 수정·교체해 다시 빌드할 수 있습니다(LGPL-2.1 §6).
- 그 밖의 서드파티 라이선스는 [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)를 보세요.

## Python 버전

v0.8.7 이하는 Python(CustomTkinter + rawpy) 앱이었습니다. 예전 실행 파일은 [Releases](https://github.com/marushin/CcoonVerter/releases)의 v0.8.x 항목에서 받을 수 있습니다. 자세한 안내: [legacy/README.md](legacy/README.md).

---

## English

**CcooNverter** is a fast batch RAW → JPG converter for Windows and macOS. Pick a folder, press one button, and every RAW file (sub-folders included) becomes a JPG. Version 0.9.0 is a complete rewrite in Rust and ships as a single portable executable (Windows) or a universal app/DMG (macOS).

- **Download**: [Releases](https://github.com/marushin/CcoonVerter/releases/latest) — `CcooNverter_v0.10.0_windows_x64.exe` or `CcooNverter_v0.10.0_macos_universal.dmg`. The macOS app is not notarized: right-click → Open the first time, or run `xattr -dr com.apple.quarantine /Applications/CcooNverter.app`.
- **Features**: live preview with before/after against the camera JPEG, folder and file drag and drop, default filmic tone curve, EV exposure control, 12 picture styles, brightness / RGB balance / levels / white balance, fast embedded-JPEG extraction (lossless copy when not resizing), resize and quality, EXIF preservation, presets (compatible with the Python version), multi-core batches with progress, ETA and cancel, "open output folder", optional update check.
- **Formats**: ARW, CR2, CR3, NEF, DNG (decoded with [rawler](https://github.com/dnglab/dnglab)).
- **Settings**: OS standard folders (see the table above); legacy Python settings next to the executable are imported once; set `CCOONVERTER_HOME` for a portable setup.
- **Build**: `cargo build --release -p ccoonverter` (OpenGL/glow renderer; add `--features wgpu` for the wgpu renderer), `scripts/build_windows.ps1`, `scripts/build_macos.sh`. A CLI (`ccoonverter-cli convert|scan|info`) shares the same engine.
- **License**: MIT. rawler is LGPL-2.1; see [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).
- The Python version (≤ v0.8.7) remains available from the v0.8.x releases.
