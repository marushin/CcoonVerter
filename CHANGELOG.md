# Changelog

이 파일의 `## vX.Y.Z - 날짜` 절은 GitHub 릴리스 본문으로 그대로 사용됩니다(`.github/workflows/release.yml`).

## v0.10.0 - 2026-11-09

**See the result before converting: live preview with camera-JPEG comparison, filmic tone curve and EV exposure, drag and drop.**

### 새 기능

1. **미리보기 패널**: 원본 폴더의 첫 RAW 파일을 축소 현상해 보여 주고, 이전/다음 버튼으로 다른 파일로 이동합니다. 슬라이더나 스타일을 바꾸면 캐시한 현상 결과에 조정 단계만 다시 적용합니다(1600px 기준 약 20ms). 슬라이더를 움직이는 동안에는 항상 최신 값만 계산하므로 화면이 멈추지 않습니다.
2. **변환 결과와 같은 계산**: 미리보기는 배치 변환과 같은 함수 체인을 씁니다. 보이는 결과가 저장되는 결과입니다.
3. **before/after 비교**: 카메라가 RAW에 넣어 둔 JPEG과 현상 결과를 버튼 하나로 비교합니다(임베디드 JPEG이 있는 파일만). "Extract Embedded JPEG" 모드에서는 임베디드 JPEG만 표시합니다.
4. **레이아웃**: 넓은 창(1040pt 이상)에서는 설정과 미리보기가 나란히 표시되고, 좁은 창에서는 SHOW PREVIEW 버튼으로 펼칩니다. 기본 창 크기(600x820)에서는 모든 설정이 스크롤 없이 보입니다.
5. **Exposure (EV) 슬라이더**: -3~+3 범위의 노출 보정을 RAW ADJUSTMENTS에 추가했습니다. 기존 Brightness와 같은 선형 단계에서 곱해지며, Brightness 2.0은 정확히 +1 EV입니다.
6. **기본 톤 커브**: 카메라 JPEG에 가까운 대비를 주는 필름풍 S 커브를 기본으로 적용합니다. RAW ADJUSTMENTS의 "Tone Curve"로 끌 수 있습니다.
7. **드래그 앤 드롭**: 폴더를 창에 놓으면 원본 폴더로 지정되고, RAW 파일을 놓으면 그 파일들만 다음 변환 대상으로 선택됩니다(공통 상위 폴더가 원본 기준, 출력 규칙은 같음). RAW가 아닌 파일은 메시지와 함께 무시합니다. 파일을 끌어오면 창 전체에 드롭 표시가 나타납니다.
8. **명령줄**: `--ev`, `--no-tone-curve`, `--sharpen` 옵션을 추가했습니다.

### 변경된 동작

- 현상 기본값이 바뀌었습니다: 톤 커브 켜짐, 약한 출력 샤프닝 적용. 예전 설정과 프리셋은 그대로 읽히지만 새 기본값으로 현상되어 v0.9.0과 결과가 달라 보일 수 있습니다.
- 클리핑된 하이라이트를 흰색 쪽으로 정리해 색이 깨지지 않도록 했습니다. 샤프닝은 출력 해상도에서 마지막에 적용합니다.

### 측정

샘플 4종의 평균 휘도 차이(카메라 JPEG 대비, 현상 결과 - 카메라 JPEG): R6 +16.1% → +6.9%, D750 +77.8% → +33.0%로 나아졌고, 7D Mark II는 +24.1% → +25.1%로 거의 같고, Sony A7S는 -12.8% → -18.6%로 나빠졌습니다. 상세 수치는 `docs/V0.10_REPORT.md`에 있습니다.

### 알려진 제한

- 자동 노출은 v0.9.0과 같은 상위 1% 기준입니다. 저조도 장면은 카메라 JPEG보다 밝게, 일부 장면은 어둡게 나올 수 있습니다.
- 파일을 드롭하면 바로 변환하지 않고 대상으로 선택만 합니다. 변환은 메인 버튼으로 시작합니다.
- 실제 OS 드래그 앤 드롭은 자동으로 검증하지 못했습니다(드롭 처리 로직은 단위 테스트로 확인). macOS는 이 버전에서 실행 확인을 하지 않았습니다.

## v0.9.0 - 2026-11-09

**Complete rewrite in Rust: Windows + macOS (universal) builds, settings moved to OS folders (auto-imported), lossless embedded-JPEG copy, no-overwrite output naming.**

### Rust 전면 재작성

- Python(CustomTkinter + rawpy/LibRaw) 앱을 Rust로 다시 만들었습니다. GUI는 egui/eframe, RAW 디코딩은 순수 Rust 라이브러리 rawler를 사용합니다.
- Windows는 설치 없는 단일 실행 파일(`CcooNverter_v0.9.0_windows_x64.exe`, 약 16MB), macOS는 Apple Silicon + Intel universal 앱과 DMG(`CcooNverter_v0.9.0_macos_universal.dmg`)로 배포합니다.
- 화면 구성, 문구, 색상, 버튼 순서는 Python v0.8.7과 같습니다. 12가지 픽처 스타일, 밝기/RGB/Levels/화이트 밸런스, 리사이즈, 품질, EXIF, 프리셋, 진행률/ETA/취소, 업데이트 알림을 모두 지원합니다.
- 같은 엔진을 쓰는 명령줄 도구 `ccoonverter-cli`(`convert`, `scan`, `info`, `--json`)를 추가했습니다(소스 빌드).

### 새 기능 / 변경된 동작

1. **설정 위치 변경 + 자동 이전**: 설정·프리셋·로그를 실행 폴더 대신 OS 표준 위치에 저장합니다(Windows `%APPDATA%\ccoon\CcooNverter`, macOS `~/Library/Application Support/ccoon.CcooNverter`). 실행 파일과 같은 폴더에 Python 버전의 `converter_settings.json`/`presets.json`이 있으면 첫 실행 때 한 번 가져옵니다. `CCOONVERTER_HOME` 환경 변수로 포터블 모드를 쓸 수 있습니다.
2. **임베디드 JPEG 무손실 복사**: "Extract Embedded JPEG"에서 리사이즈가 0이면 다시 압축하지 않고 카메라 JPEG 바이트를 그대로 저장합니다(EXIF 끄면 메타데이터만 제거). 리사이즈하면 이전처럼 다시 인코딩합니다.
3. **출력 파일 충돌 정책**: 같은 이름의 JPG가 있으면 덮어쓰지 않고 접미사를 붙입니다(`IMG_1_arw.jpg`, 그다음 `IMG_1_arw_1.jpg`, `_2` …). 한 배치 안에서 `IMG_1.ARW`와 `IMG_1.DNG`가 겹치면 두 번째는 항상 `IMG_1_dng.jpg`가 됩니다. DIRECTORY PATHS의 "If output exists"에서 Add suffix / Skip / Overwrite를 고를 수 있습니다.
4. **사진 방향**: 현상하거나 다시 인코딩한 이미지는 픽셀을 똑바로 회전하고 EXIF Orientation을 1로 기록합니다(예전에는 일부 뷰어에서 두 번 회전되거나 임베디드 미리보기가 누워 있었음). 무손실 복사는 바이트를 유지하고 방향을 EXIF로 전달합니다.
5. **톤 조정 정밀도**: 밝기, RGB, Levels, 스타일을 32비트 부동소수점으로 계산해 밴딩이 줄었습니다. 현상 엔진이 LibRaw에서 rawler로 바뀌어 색과 밝기가 Python 버전과 미세하게 다를 수 있습니다(자동 노출은 상위 1% 클립 기준).
6. **업데이트 확인 끄기**: 창 아래 "Check for updates on startup"으로 끌 수 있고, 확인 결과는 24시간 캐시합니다.
7. **출력 폴더 열기**: 변환 완료 대화상자에서 바로 출력 폴더를 열 수 있습니다.
8. **스레드 수 기본값**: 임베디드 모드는 모든 코어, 현상 모드는 메모리를 고려해 최대 4개를 사용합니다.
9. **마지막 원본 폴더 기억**: 다음 실행 때 마지막으로 쓴 원본 폴더를 다시 채웁니다(폴더가 남아 있을 때).
10. **로그**: `converter.log` 대신 OS 데이터 폴더의 `logs/ccoonverter.log`에 기록하고 5MB를 넘으면 교체합니다.
11. **Windows 다크 타이틀바**: 제목 표시줄을 앱과 같은 어두운 색(골드 제목)으로 표시합니다.
12. 지원 확장자는 Python과 같은 5종(ARW, CR2, CR3, NEF, DNG)입니다.

### 알려진 제한

- macOS 앱은 Apple 공증을 받지 않았습니다(ad-hoc 서명). 처음 실행할 때 우클릭 → 열기 또는 `xattr -dr com.apple.quarantine /Applications/CcooNverter.app`이 필요합니다. 관리되는 회사 Mac에서는 실행이 막힐 수 있습니다.
- macOS 빌드는 CI에서 만들며 Intel Mac 실기기 테스트는 하지 않았습니다. macOS 11 미만은 지원하지 않습니다.
- 실제 파일 검증은 ARW(ILCE-7S), CR2(7D Mark II), CR3(R6), NEF(D750), DNG(5D Mark III) 각 1종입니다. Nikon Z의 HE/HE* NEF, Sony 신형 무손실 압축 ARW, 세로(Orientation 6/8) 실제 샘플은 검증하지 않았습니다.
- 현상 결과의 색은 카메라 JPEG이나 LibRaw와 완전히 같지 않습니다(카메라 톤 커브 미적용).
- JPEG 최적화 허프만 테이블을 쓰지 않아 Python 버전보다 파일이 몇 % 클 수 있습니다.
- 기본 렌더러는 OpenGL 2.0 이상이 필요합니다. 아주 오래된 그래픽 드라이버에서는 `--features wgpu` 빌드를 쓰세요.
- 네이티브 확인 대화상자(예: 취소 확인)가 떠 있는 동안에는 진행률 표시가 갱신되지 않습니다(변환은 계속됨).
