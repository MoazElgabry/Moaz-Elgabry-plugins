import test from "node:test";
import assert from "node:assert/strict";
import { extractDiagnostics, parseBooleanMarker } from "./generate-plugin-channel-manifests.mjs";

test("extractDiagnostics parses hidden JSON comment", () => {
  const body = `
Visible release notes.

<!-- manager-diagnostics
{
  "enabled": true,
  "logSourcePath": {
    "windows": "%LOCALAPPDATA%\\\\LensDiff\\\\Logs",
    "macos": "~/Library/Logs/MoazElgabry/LensDiff/LensDiffTiming.log"
  },
  "environment": {
    "LENSDIFF_LOG": "1",
    "LENSDIFF_TIMING": "1"
  }
}
-->
`;

  assert.deepEqual(extractDiagnostics(body, "LensDiff v1"), {
    enabled: true,
    logSourcePath: {
      windows: "%LOCALAPPDATA%\\LensDiff\\Logs",
      macos: "~/Library/Logs/MoazElgabry/LensDiff/LensDiffTiming.log"
    },
    environment: {
      LENSDIFF_LOG: "1",
      LENSDIFF_TIMING: "1"
    }
  });
});

test("extractDiagnostics accepts multiple log sources per platform", () => {
  const body = `
<!-- manager-diagnostics
{
  "enabled": true,
  "logSourcePath": {
    "windows": [
      "%LOCALAPPDATA%\\\\Chromaspace.log",
      "%LOCALAPPDATA%\\\\Chromaspace_CubeViewer.log"
    ],
    "macos": "~/Library/Logs/Chromaspace.log",
    "linux": "~/.cache/Chromaspace.log"
  },
  "environment": {
    "CHROMASPACE_DEBUG_LOG": "1",
    "CHROMASPACE_MULTI_INSTANCE_DEBUG": "1",
    "CHROMASPACE_DIAGNOSTICS": "1"
  }
}
-->
`;

  assert.deepEqual(extractDiagnostics(body, "Chromaspace v1"), {
    enabled: true,
    logSourcePath: {
      windows: [
        "%LOCALAPPDATA%\\Chromaspace.log",
        "%LOCALAPPDATA%\\Chromaspace_CubeViewer.log"
      ],
      macos: "~/Library/Logs/Chromaspace.log",
      linux: "~/.cache/Chromaspace.log"
    },
    environment: {
      CHROMASPACE_DEBUG_LOG: "1",
      CHROMASPACE_MULTI_INSTANCE_DEBUG: "1",
      CHROMASPACE_DIAGNOSTICS: "1"
    }
  });
});

test("extractDiagnostics accepts disabled empty metadata block", () => {
  const body = `
<!-- manager-diagnostics
{
  "enabled": false,
  "logSourcePath": {},
  "environment": {}
}
-->
`;

  assert.deepEqual(extractDiagnostics(body, "Plugin v1"), {
    enabled: false,
    logSourcePath: {},
    environment: {}
  });
});

test("extractDiagnostics parses ME_OpenDRT default diagnostics metadata", () => {
  const body = `
<!-- manager-diagnostics
{
  "enabled": false,
  "logSourcePath": {
    "windows": [
      "%LOCALAPPDATA%\\\\ME_OpenDRT",
      "%TEMP%\\\\ME_OpenDRT_CubeViewer.log"
    ],
    "macos": [
      "~/Library/Logs/ME_OpenDRT.log",
      "~/Library/Logs/ME_OpenDRT_CubeViewer.log"
    ],
    "linux": [
      "~/.cache/ME_OpenDRT",
      "~/.cache/ME_OpenDRT_CubeViewer.log"
    ]
  },
  "environment": {
    "ME_OPENDRT_DEBUG_LOG": "1",
    "ME_OPENDRT_PERF_LOG": "1",
    "ME_OPENDRT_VIEWER_DIAGNOSTICS": "1"
  }
}
-->
`;

  assert.deepEqual(extractDiagnostics(body, "ME_OpenDRT v1"), {
    enabled: false,
    logSourcePath: {
      windows: [
        "%LOCALAPPDATA%\\ME_OpenDRT",
        "%TEMP%\\ME_OpenDRT_CubeViewer.log"
      ],
      macos: [
        "~/Library/Logs/ME_OpenDRT.log",
        "~/Library/Logs/ME_OpenDRT_CubeViewer.log"
      ],
      linux: [
        "~/.cache/ME_OpenDRT",
        "~/.cache/ME_OpenDRT_CubeViewer.log"
      ]
    },
    environment: {
      ME_OPENDRT_DEBUG_LOG: "1",
      ME_OPENDRT_PERF_LOG: "1",
      ME_OPENDRT_VIEWER_DIAGNOSTICS: "1"
    }
  });
});

test("extractDiagnostics omits missing diagnostics block", () => {
  assert.equal(extractDiagnostics("Visible release notes only.", "LensDiff v1"), undefined);
});

test("extractDiagnostics rejects malformed diagnostics JSON", () => {
  assert.throws(
    () => extractDiagnostics("<!-- manager-diagnostics\n{ nope }\n-->", "LensDiff v1"),
    /valid JSON/
  );
});

test("extractDiagnostics rejects empty log source arrays", () => {
  const body = `
<!-- manager-diagnostics
{
  "enabled": true,
  "logSourcePath": {
    "windows": []
  },
  "environment": {}
}
-->
`;

  assert.throws(
    () => extractDiagnostics(body, "Plugin v1"),
    /at least one path/
  );
});

test("parseBooleanMarker supports hidden and legacy rollback markers", () => {
  assert.equal(parseBooleanMarker("manager-available-stable: true", "manager-available-stable"), true);
  assert.equal(parseBooleanMarker("<!-- manager-available-stable: true -->", "manager-available-stable"), true);
  assert.equal(parseBooleanMarker("<!-- manager-available-stable: false -->", "manager-available-stable"), false);
});
