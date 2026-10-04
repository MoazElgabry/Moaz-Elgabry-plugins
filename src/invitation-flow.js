export function classifyInvitationVerificationResponse(response) {
  if (response?.token) return "connected";
  if (response?.receipt) return "pending";
  return "missing-reference";
}

export function invitationRequestAction(status, receipt) {
  if (status === "approved_key_confirmation") return "forget-local";
  if (status === "pending" || status === "verification_required") {
    return receipt ? "withdraw" : "forget-local";
  }
  return "forget-invitation";
}
