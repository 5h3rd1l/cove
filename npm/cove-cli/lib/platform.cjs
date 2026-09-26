const PLATFORM_PACKAGES = Object.freeze({
  "darwin-arm64": "cove-cli-darwin-arm64",
  "darwin-x64": "cove-cli-darwin-x64",
  "linux-arm64": "cove-cli-linux-arm64",
  "linux-x64": "cove-cli-linux-x64",
});

function packageFor(platform = process.platform, arch = process.arch) {
  const packageName = PLATFORM_PACKAGES[`${platform}-${arch}`];
  if (packageName) {
    return packageName;
  }

  const supported = Object.keys(PLATFORM_PACKAGES).join(", ");
  throw new Error(
    `cove does not support ${platform}-${arch}. Supported platforms: ${supported}`,
  );
}

function resolveBinary(platform = process.platform, arch = process.arch) {
  const packageName = packageFor(platform, arch);
  const executable = "cove";

  try {
    return require.resolve(`${packageName}/bin/${executable}`);
  } catch (cause) {
    throw new Error(
      `The native package ${packageName} is missing. Reinstall cove-cli without omitting optional dependencies.`,
      { cause },
    );
  }
}

module.exports = { PLATFORM_PACKAGES, packageFor, resolveBinary };
