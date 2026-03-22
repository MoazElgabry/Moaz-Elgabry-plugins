import fs from "node:fs";
import path from "node:path";

function parseArgs(argv) {
  const args = {
    managerRoot: process.cwd(),
    configs: []
  };

  for (let i = 0; i < argv.length; i += 1) {
    const current = argv[i];
    if (current === "--manager-root") {
      args.managerRoot = argv[++i];
      continue;
    }
    if (current === "--config") {
      args.configs.push(argv[++i]);
      continue;
    }
    throw new Error(`Unknown argument: ${current}`);
  }

  return args;
}

function discoverDefaultConfigs(managerRoot) {
  const gitRoot = path.resolve(managerRoot, "..");
  const candidates = [
    path.join(gitRoot, "Chromaspace", "manager-release-config.json"),
    path.join(gitRoot, "ME_OFX", "manager-release-config.json")
  ];
  return candidates.filter((candidate) => fs.existsSync(candidate));
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function writeJson(filePath, value) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function validatePlatformPackage(pluginId, version, pkg) {
  const required = [
    "platform",
    "arch",
    "downloadUrl",
    "sha256",
    "packageType",
    "bundleName",
    "bundleIdentifier",
    "installPath",
    "minManagerVersion",
    "hostProcesses"
  ];

  for (const field of required) {
    assert(pkg[field] !== undefined && pkg[field] !== null, `${pluginId} ${version}: missing platform field '${field}'`);
  }

  assert(Array.isArray(pkg.hostProcesses) && pkg.hostProcesses.length > 0, `${pluginId} ${version}: hostProcesses must be a non-empty array`);
}

function validateRelease(pluginId, version, release) {
  assert(release, `${pluginId}: missing release metadata for ${version}`);
  assert(typeof release.releaseDate === "string" && release.releaseDate.length > 0, `${pluginId} ${version}: releaseDate is required`);
  assert(typeof release.releaseNotesUrl === "string" && release.releaseNotesUrl.length > 0, `${pluginId} ${version}: releaseNotesUrl is required`);
  assert(Array.isArray(release.platforms) && release.platforms.length > 0, `${pluginId} ${version}: platforms must be a non-empty array`);
  release.platforms.forEach((pkg) => validatePlatformPackage(pluginId, version, pkg));
}

function buildRelease(version, config) {
  const release = config.releases[version];
  validateRelease(config.pluginId, version, release);
  return {
    version,
    releaseDate: release.releaseDate,
    releaseNotesUrl: release.releaseNotesUrl,
    platforms: release.platforms
  };
}

function buildStableManifest(config) {
  assert(typeof config.pluginId === "string" && config.pluginId.length > 0, "pluginId is required");
  assert(typeof config.displayName === "string" && config.displayName.length > 0, `${config.pluginId}: displayName is required`);
  assert(typeof config.stable === "string" && config.stable.length > 0, `${config.pluginId}: stable version is required`);
  assert(Array.isArray(config.available_stable), `${config.pluginId}: available_stable must be an array`);
  assert(config.releases && typeof config.releases === "object", `${config.pluginId}: releases map is required`);

  const stableRelease = buildRelease(config.stable, config);
  const availableVersions = config.available_stable
    .filter((version, index, array) => typeof version === "string" && version !== config.stable && array.indexOf(version) === index)
    .map((version) => buildRelease(version, config));

  return {
    pluginId: config.pluginId,
    displayName: config.displayName,
    version: stableRelease.version,
    releaseDate: stableRelease.releaseDate,
    releaseNotesUrl: stableRelease.releaseNotesUrl,
    platforms: stableRelease.platforms,
    availableVersions
  };
}

function buildBetaManifest(config) {
  if (!config.beta) {
    return null;
  }

  const betaRelease = buildRelease(config.beta, config);
  return {
    pluginId: config.pluginId,
    displayName: config.displayName,
    version: betaRelease.version,
    releaseDate: betaRelease.releaseDate,
    releaseNotesUrl: betaRelease.releaseNotesUrl,
    platforms: betaRelease.platforms,
    availableVersions: []
  };
}

function updateIndex(indexPath, pluginId, displayName) {
  const index = readJson(indexPath);
  index.generatedAt = new Date().toISOString();
  const manifestUrl = `https://moazelgabry.github.io/Moaz-Elgabry-plugins/plugins/${pluginId}/stable.json`;
  const entries = Array.isArray(index.plugins) ? [...index.plugins] : [];
  const existingIndex = entries.findIndex((entry) => entry.pluginId === pluginId);
  const nextEntry = { pluginId, displayName, manifestUrl };

  if (existingIndex >= 0) {
    entries[existingIndex] = nextEntry;
  } else {
    entries.push(nextEntry);
  }

  entries.sort((left, right) => left.displayName.localeCompare(right.displayName));
  index.plugins = entries;
  writeJson(indexPath, index);
}

function removeIfExists(filePath) {
  if (fs.existsSync(filePath)) {
    fs.unlinkSync(filePath);
  }
}

function generateForConfig(configPath, managerRoot) {
  const config = readJson(configPath);
  const pluginDir = path.join(managerRoot, "docs", "plugins", config.pluginId);
  const stablePath = path.join(pluginDir, "stable.json");
  const betaPath = path.join(pluginDir, "beta.json");
  const indexPath = path.join(managerRoot, "docs", "plugins", "index.json");

  const stableManifest = buildStableManifest(config);
  const betaManifest = buildBetaManifest(config);

  writeJson(stablePath, stableManifest);
  if (betaManifest) {
    writeJson(betaPath, betaManifest);
  } else {
    removeIfExists(betaPath);
  }
  updateIndex(indexPath, config.pluginId, config.displayName);

  console.log(`Generated manifests for ${config.pluginId} from ${configPath}`);
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const managerRoot = path.resolve(args.managerRoot);
  const configs = args.configs.length ? args.configs.map((item) => path.resolve(item)) : discoverDefaultConfigs(managerRoot);

  assert(configs.length > 0, "No manager-release-config.json files were provided or discovered.");

  for (const configPath of configs) {
    generateForConfig(configPath, managerRoot);
  }
}

main();
