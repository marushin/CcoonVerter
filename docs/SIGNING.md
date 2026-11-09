# 코드 서명과 공증 준비

`.github/workflows/release.yml`에는 macOS 공증과 Windows 서명 단계가 들어 있습니다. **시크릿이 전부 등록된 경우에만 실행되고**, 없으면 그 단계들은 건너뜁니다. 시크릿이 없는 저장소의 빌드는 지금과 똑같습니다. 릴리스는 계속 `draft: true`로 만들어지므로, 시크릿을 등록해도 자동으로 공개되지 않습니다.

> 이 단계들은 이 저장소에서 실행해 본 적이 없습니다(개발 PC에는 Apple 인증서와 Azure 계정이 없고 CI 실행도 하지 않음). YAML 구문만 확인했습니다. 시크릿을 등록한 뒤 첫 태그 빌드에서 로그를 꼭 확인하세요.

시크릿은 저장소 **Settings → Secrets and variables → Actions → New repository secret**에서 등록합니다. 이름은 아래 표와 정확히 같아야 합니다. 일부만 등록하면 해당 OS의 서명 단계 전체가 건너뛰어집니다.

## macOS: Developer ID 서명과 공증

준비물: Apple Developer Program 계정(연간 유료), **Developer ID Application** 인증서.

| 시크릿 | 내용 | 얻는 방법 |
|---|---|---|
| `APPLE_CERT_P12_BASE64` | Developer ID Application 인증서(개인 키 포함)의 `.p12` 파일을 base64로 인코딩한 문자열 | 키체인 접근에서 인증서를 `.p12`로 내보낸 뒤 `base64 -i cert.p12 \| pbcopy` |
| `APPLE_CERT_PASSWORD` | `.p12`를 내보낼 때 지정한 암호 | 내보낼 때 직접 지정 |
| `APPLE_ID` | Apple 계정 이메일 | |
| `APPLE_TEAM_ID` | 10자리 Team ID | developer.apple.com → Membership |
| `APPLE_APP_PASSWORD` | 앱 전용 암호(계정 암호가 아님) | appleid.apple.com → 로그인 및 보안 → 앱 암호 |

흐름: 임시 키체인에 인증서 가져오기 → hardened runtime으로 `.app` 서명 → 서명된 앱으로 DMG 다시 만들기 → DMG 서명 → `notarytool`로 제출 및 대기 → 앱과 DMG에 공증 결과 스테이플 → `spctl`로 검사 → 임시 키체인 삭제.

공증이 거부되면 `xcrun notarytool log <submission-id> --apple-id ... --team-id ... --password ...`로 이유를 볼 수 있습니다. 현재 앱은 특별한 entitlement 없이 서명합니다. 실행 중 권한 오류가 나면 entitlements 파일이 필요할 수 있습니다.

공증을 마치면 `README.md`의 "macOS 첫 실행" 안내(우클릭 → 열기)는 지워도 됩니다.

## Windows: Azure Trusted Signing

Microsoft는 이 서비스를 현재 **Artifact Signing**이라는 이름으로 제공합니다. 워크플로는 공식 액션 `Azure/artifact-signing-action@v2.0.0`을 사용합니다.

준비물: Azure 구독, Trusted Signing 계정과 인증서 프로필(신원 확인 필요), 서명 권한(`Trusted Signing Certificate Profile Signer` 역할)을 준 앱 등록(서비스 주체).

| 시크릿 | 내용 |
|---|---|
| `AZURE_TENANT_ID` | Microsoft Entra 테넌트 ID |
| `AZURE_CLIENT_ID` | 앱 등록의 클라이언트 ID |
| `AZURE_CLIENT_SECRET` | 앱 등록의 클라이언트 시크릿 |
| `AZURE_SIGNING_ENDPOINT` | 계정 지역의 엔드포인트, 예: `https://wus2.codesigning.azure.net/` |
| `AZURE_SIGNING_ACCOUNT` | Trusted Signing 계정 이름 |
| `AZURE_CERT_PROFILE` | 인증서 프로필 이름 |

서명 대상은 `target/release/CcooNverter.exe`이고, 이름을 바꿔 아티팩트로 올리기 전에 서명합니다. 다이제스트는 SHA256, 타임스탬프는 `http://timestamp.acs.microsoft.com`을 씁니다.

## 확인 방법

- 시크릿 없는 저장소: Actions 로그에서 서명 단계가 회색(skipped)으로 표시되어야 합니다.
- 서명 후 Windows: 파일 속성 → 디지털 서명, 또는 `signtool verify /pa CcooNverter_vX.Y.Z_windows_x64.exe`.
- 서명 후 macOS: `spctl --assess --type execute --verbose=2 CcooNverter.app`, `xcrun stapler validate CcooNverter.app`.
