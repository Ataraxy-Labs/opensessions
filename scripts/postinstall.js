const fs = require("fs");
const https = require("https");
const os = require("os");
const path = require("path");
const { execFileSync } = require("child_process");

const REPO_RELEASE_BASE = "https://github.com/ataraxy-labs/opensessions/releases/download";

function targetTriple(platform = os.platform(), arch = os.arch()) {
  if (platform === "darwin" && arch === "arm64") return "aarch64-apple-darwin";
  if (platform === "darwin" && arch === "x64") return "x86_64-apple-darwin";
  if (platform === "linux" && arch === "x64") return "x86_64-unknown-linux-gnu";
  if (platform === "linux" && arch === "arm64") return "aarch64-unknown-linux-gnu";
  if (platform === "win32" && arch === "x64") return "x86_64-pc-windows-msvc";
  throw new Error(`Unsupported platform: ${platform}-${arch}`);
}

function artifactName(triple) {
  return `opensessions-sidebar-${triple}.tar.gz`;
}

function releaseUrl(version, triple) {
  return `${REPO_RELEASE_BASE}/v${version}/${artifactName(triple)}`;
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const file = fs.createWriteStream(dest);
    https.get(url, (response) => {
      if (response.statusCode && response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
        file.close(() => fs.unlink(dest, () => {}));
        download(response.headers.location, dest).then(resolve, reject);
        return;
      }

      if (response.statusCode !== 200) {
        file.close(() => fs.unlink(dest, () => {}));
        reject(new Error(`Download failed: ${response.statusCode} ${url}`));
        return;
      }

      response.pipe(file);
      file.on("finish", () => file.close(resolve));
    }).on("error", (err) => {
      file.close(() => fs.unlink(dest, () => {}));
      reject(err);
    });
  });
}

async function main() {
  const { version } = require("../package.json");
  const triple = targetTriple();
  const binDir = path.join(__dirname, "..", "bin");
  const dest = path.join(binDir, process.platform === "win32" ? "opensessions-sidebar.exe" : "opensessions-sidebar");
  const tarball = path.join(binDir, "opensessions-sidebar.tmp.tar.gz");

  fs.mkdirSync(binDir, { recursive: true });
  await download(releaseUrl(version, triple), tarball);
  execFileSync("tar", ["-xzf", tarball, "-C", binDir]);
  fs.unlinkSync(tarball);
  fs.chmodSync(dest, 0o755);
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}

module.exports = { artifactName, releaseUrl, targetTriple };
