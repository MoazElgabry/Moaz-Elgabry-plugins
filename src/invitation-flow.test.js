import assert from "node:assert/strict";
import test from "node:test";
import { classifyInvitationVerificationResponse, invitationRequestAction } from "./invitation-flow.js";

test("a verified request receipt means the request is pending approval", () => {
  assert.equal(
    classifyInvitationVerificationResponse({ receipt: "mrr_test_receipt", status: "pending" }),
    "pending"
  );
});

test("an invitation token means the request can connect immediately", () => {
  assert.equal(
    classifyInvitationVerificationResponse({ token: "mer_test_token", receipt: "mrr_test_receipt" }),
    "connected"
  );
});

test("a response without a token or receipt cannot be tracked", () => {
  assert.equal(classifyInvitationVerificationResponse({ status: "pending" }), "missing-reference");
});

test("a pending request without its receipt can only be forgotten locally", () => {
  assert.equal(invitationRequestAction("pending", ""), "forget-local");
});

test("a pending request with its receipt can be withdrawn remotely", () => {
  assert.equal(invitationRequestAction("pending", "mrr_test_receipt"), "withdraw");
});
