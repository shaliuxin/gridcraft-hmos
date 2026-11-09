#!/usr/bin/env bash
# 为 gridcraft-hmos 生成一份【本地】调试签名（OpenHarmony 本地 CA 链），
# 绑独立 bundle 名 io.github.shaliuxin.gridcraft.hmos，不与其它 craft 项目的
# AGC profile 冲突（包名铁律：每个项目用自己的 bundleName + 专属签名产物）。
# 产出 signing/ 下的 .cer/.p12/.p7b，供 build-profile.json5 引用。
#
# 注意：这是本地自签 CA 链，产出的 profile 是 issuer="pki_internal"。
# hvigor SignHap 接受它（可完整验证打包链路），但 HarmonyOS 真机通常只信任
# AGC 签发的 issuer="app_gallery" 调试 profile。真机安装请在 DevEco Studio
# 「File → Project Structure → Signing Configs」勾选自动签名，让它为本包名
# io.github.shaliuxin.gridcraft.hmos 签发专属 profile 并回写 build-profile.json5。
#
# 用法（本机已实测跑通）：
#   GC_KEYPW/GC_STOREPW 必须与 build-profile.json5 中 keyPassword/storePassword
#   解出的明文一致 —— hvigor 用 storeFile 同级的 signing/material/ 解密，不是
#   ~/.ohos/config/material。换口令时用 node scripts/encrypt_pwd.js <pwd> 重生成 hex。
#   GC_UID1/GC_UID2 取设备 UDID：hdc shell "bm get --udid"。
#   export GC_KEYPW=... GC_STOREPW=... GC_UID1=... GC_UID2=...
#   bash scripts/make_gridcraft_signing.sh
# 切勿把真实口令/UDID 写回脚本或提交到公开仓库。
set -euo pipefail

DEVECO="${DEVECO_HOME:-/c/Program Files/Huawei/DevEco Studio}"
JAVA="$DEVECO/jbr/bin/java.exe"
SIGTOOL="$DEVECO/sdk/default/openharmony/toolchains/lib/hap-sign-tool.jar"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/signing"
BUNDLE="${GC_BUNDLE:-io.github.shaliuxin.gridcraft.hmos}"
KEYALIAS="gridcraft-debug"
# 口令与设备 UDID 不入库：从环境变量读取（本地调试时自行 export）。
#   export GC_KEYPW=... GC_STOREPW=... GC_UID1=<设备UDID> GC_UID2=<设备UDID>
# 切勿把真实口令/UDID 写回脚本或提交到公开仓库。
KEYPW="${GC_KEYPW:?请 export GC_KEYPW（本地调试 CA 密钥口令，勿入库）}"
STOREPW="${GC_STOREPW:?请 export GC_STOREPW（本地调试 CA 库口令，勿入库）}"
UID1="${GC_UID1:?请 export GC_UID1（设备 UDID，勿入库）}"
UID2="${GC_UID2:?请 export GC_UID2（设备 UDID，勿入库）}"

mkdir -p "$OUT"
cd "$OUT"
echo "bundle=$BUNDLE out=$OUT"
run() { echo "+ $JAVA -jar $SIGTOOL $*"; "$JAVA" -jar "$SIGTOOL" "$@" 2>&1 | grep -iE "success|error|exception" | tail -2 || true; }

# 1) 根 CA 密钥对
rm -f root.jks root-ca.cer app-sub.jks app-sub-ca.cer app.p12 app-cert.cer profile.jks profile-ca.cer profile-cert.cer app-profile.json app-leaf.pem app-leaf.der gridcraft-debug.p7b
run generate-keypair -keyAlias oh-root-ca -keyAlg ECC -keySize NIST-P-256 \
  -keystoreFile root.jks -keystorePwd "$STOREPW" -keyPwd "$KEYPW"

# 2) 自签根证书
run generate-ca -keyAlias oh-root-ca -keyAlg ECC -keySize NIST-P-256 -signAlg SHA256withECDSA \
  -subject "C=CN,O=gridcraft,OU=dev,CN=GridCraft Root CA" \
  -keystoreFile root.jks -keystorePwd "$STOREPW" -keyPwd "$KEYPW" \
  -outFile root-ca.cer

# 3) 应用签名子 CA 密钥对（由根 CA 签发）
run generate-keypair -keyAlias oh-app-sub-ca -keyAlg ECC -keySize NIST-P-256 \
  -keystoreFile app-sub.jks -keystorePwd "$STOREPW" -keyPwd "$KEYPW"
run generate-ca -keyAlias oh-app-sub-ca -keyAlg ECC -keySize NIST-P-256 -signAlg SHA256withECDSA \
  -issuer "C=CN,O=gridcraft,OU=dev,CN=GridCraft Root CA" \
  -issuerKeyAlias oh-root-ca -issuerKeyPwd "$KEYPW" -issuerKeystorePwd "$STOREPW" -issuerKeystoreFile root.jks \
  -subject "C=CN,O=gridcraft,OU=dev,CN=GridCraft App Signing CA" \
  -keystoreFile app-sub.jks -keystorePwd "$STOREPW" -keyPwd "$KEYPW" \
  -outFile app-sub-ca.cer

# 4) 应用密钥对（p12，供 hvigor storeFile）
run generate-keypair -keyAlias "$KEYALIAS" -keyAlg ECC -keySize NIST-P-256 \
  -keystoreFile app.p12 -keystorePwd "$STOREPW" -keyPwd "$KEYPW"

# 5) 应用签名证书（由应用子 CA 签发，含 root+sub 双链）
run generate-app-cert -keyAlias "$KEYALIAS" -keyPwd "$KEYPW" \
  -issuer "C=CN,O=gridcraft,OU=dev,CN=GridCraft App Signing CA" \
  -issuerKeyAlias oh-app-sub-ca -issuerKeyPwd "$KEYPW" -issuerKeystorePwd "$STOREPW" -issuerKeystoreFile app-sub.jks \
  -subject "C=CN,O=gridcraft,OU=dev,CN=gridcraft-hmos" \
  -validity 3650 -signAlg SHA256withECDSA \
  -keystoreFile app.p12 -keystorePwd "$STOREPW" \
  -outForm certChain -rootCaCertFile root-ca.cer -subCaCertFile app-sub-ca.cer \
  -outFile app-cert.cer

# 6) profile 签名子 CA 密钥对
run generate-keypair -keyAlias oh-profile-ca -keyAlg ECC -keySize NIST-P-256 \
  -keystoreFile profile.jks -keystorePwd "$STOREPW" -keyPwd "$KEYPW"

# 7) profile 签名 CA 证书（由根 CA 签发）
run generate-ca -keyAlias oh-profile-ca -keyAlg ECC -keySize NIST-P-256 -signAlg SHA256withECDSA \
  -issuer "C=CN,O=gridcraft,OU=dev,CN=GridCraft Root CA" \
  -issuerKeyAlias oh-root-ca -issuerKeyPwd "$KEYPW" -issuerKeystorePwd "$STOREPW" -issuerKeystoreFile root.jks \
  -subject "C=CN,O=gridcraft,OU=dev,CN=GridCraft Profile CA" \
  -keystoreFile profile.jks -keystorePwd "$STOREPW" -keyPwd "$KEYPW" \
  -outFile profile-ca.cer

# 7) provision profile 证书（由 profile CA 签发，含双 CA 链）
run generate-profile-cert -keyAlias oh-profile-ca -keyPwd "$KEYPW" \
  -issuer "C=CN,O=gridcraft,OU=dev,CN=GridCraft Profile CA" \
  -issuerKeyAlias oh-profile-ca -issuerKeyPwd "$KEYPW" -issuerKeystorePwd "$STOREPW" -issuerKeystoreFile profile.jks \
  -subject "C=CN,O=gridcraft,OU=dev,CN=GridCraft Provision Profile Debug" \
  -validity 3650 -signAlg SHA256withECDSA \
  -rootCaCertFile root-ca.cer -subCaCertFile profile-ca.cer \
  -outForm certChain -keystoreFile profile.jks -keystorePwd "$STOREPW" -outFile profile-cert.cer

# 8) 生成 profile json（含 bundle 名 + 设备 UDID）
NB=$(date -u +%s); NA=$((NB + 3*365*24*3600))
# sign-app 要求 profile 的 development-certificate 是【单张 leaf】证书，且必须是
# 【DER 编码】+【URL-safe base64】(+/ 换成 -_、去 padding)。cadcraft 实测踩坑结论：
#   - 放整条链（app-cert.cer 含 leaf+子CA+根 三张）→ "Decode Base64 certificate failed"
#   - 用 PEM 文本的 base64（或含 + / 的标准 base64）→ "Illegal base64 character 2b"
#   - 正确做法：openssl 取第一张 leaf → DER → URL-safe base64；sign-app 的 -appCertFile
#     仍用整条链。wordcraft 复制来的旧写法是错的，这里已按 cadcraft 修正。
awk '/-----BEGIN CERTIFICATE-----/{f=1} f{print} /-----END CERTIFICATE-----/{exit}' app-cert.cer > app-leaf.pem
openssl x509 -in app-leaf.pem -outform DER -out app-leaf.der
APP_B64=$(python -c "import base64;print(base64.urlsafe_b64encode(open('app-leaf.der','rb').read()).decode())")
cat > app-profile.json <<EOF
{
  "version-name": "0.16.0",
  "version-code": 16000,
  "uuid": "6c1a0e2b-4f3d-4b7a-9c2e-gridcraft01",
  "validity": { "not-before": $NB, "not-after": $NA },
  "type": "debug",
  "bundle-info": {
    "developer-id": "55530755",
    "development-certificate": "$APP_B64",
    "bundle-name": "$BUNDLE",
    "apl": "normal",
    "app-feature": "hos_normal_app",
    "app-identifier": "6917620714977119980"
  },
  "baseapp-info": {},
  "permissions": {},
  "debug-info": { "device-ids": ["$UID1", "$UID2"], "device-id-type": "udid" },
  "acls": { "allowed-acls": [] },
  "app-privilege-capabilities": [],
  "issuer": "pki_internal"
}
EOF

# 9) 签 profile -> p7b
run sign-profile -mode localSign -keyAlias oh-profile-ca -keyPwd "$KEYPW" \
  -profileCertFile profile-cert.cer -inFile app-profile.json \
  -signAlg SHA256withECDSA -keystoreFile profile.jks -keystorePwd "$STOREPW" \
  -outFile gridcraft-debug.p7b

echo "=== 产物 ==="
ls -la "$OUT"/*.cer "$OUT"/*.p12 "$OUT"/*.p7b 2>&1 | tr -d '\r'
echo "=== 校验 p7b ==="
"$JAVA" -jar "$SIGTOOL" verify-profile -inFile "$OUT/gridcraft-debug.p7b" 2>&1 | grep -iE "verifiedPassed|message|bundle-name" | head
