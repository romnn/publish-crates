import * as core from "@actions/core";
import * as exec from "@actions/exec";
import {
  parseCargoPackageManifest,
  Repo,
  RustTarget,
} from "action-get-release";
import * as path from "path";

async function run(): Promise<void> {
  let version = "latest";
  let versionOverride = core.getInput("version");
  if (versionOverride && versionOverride !== "") {
    version = versionOverride;
  } else {
    try {
      // read the version from cargo manifest
      let manifest = await parseCargoPackageManifest("Cargo.toml");
      version = `v${manifest.package.version}`;
    } catch (err: unknown) {
      core.warning(`failed to read version from Cargo.toml: ${err}`);
    }
  }
  core.debug(`version=${version}`);

  const repo = new Repo();

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
