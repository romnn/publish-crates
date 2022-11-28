import core from "@actions/core";
import exec from "@actions/exec";
import {
  parseCargoPackageManifestSync,
  Repo,
  RustTarget,
} from "action-get-release";
import path from "path";

// this is a build time constant, `Cargo.toml` does not exist when the action is
// run
const manifest =
    parseCargoPackageManifestSync(path.join(__dirname, "Cargo.toml"));

function getVersion(): string {
  let version = "latest";
  let manifestVersion = manifest.package.version;
  if (manifestVersion && manifestVersion !== "") {
    version = `v${manifestVersion}`;
  }
  let versionOverride = core.getInput("version");
  if (versionOverride && versionOverride !== "") {
    version = versionOverride;
  }
  return version;
}

async function run(): Promise<void> {
  const repo = new Repo();
  const version = getVersion();
  core.debug(`version=${version}`);

  let release;
  try {
    release = version === "" || version === "latest"
                  ? await repo.getLatestRelease()
                  : await repo.getReleaseByTag(version);
  } catch (err: unknown) {
    throw new Error(
        `failed to fetch ${version} release for ${repo.fullName()}: ${err}`);
  }
  core.debug(`found ${release.assets().length} assets for ${
      version} release of ${repo.fullName()}`);

  const {platform, arch} = new RustTarget();
  core.debug(`host system: platform=${platform} arch=${arch}`);

  // publish-crates-action-x86_64-unknown-linux-gnu.tar.gz
  const asset = `publish-crates-action-${arch}-unknown-${platform}-gnu.tar.gz`;

  let downloaded;
  try {
    downloaded = await release.downloadAsset(asset, {cache : false});
  } catch (err: unknown) {
    throw new Error(`failed to download asset ${asset}: ${err}`);
  }

  // core.addPath(downloaded);
  const executable = path.join(downloaded, "publish-crates-action");
  await exec.exec(executable);
}

run().catch((error) => core.setFailed(error.message));
