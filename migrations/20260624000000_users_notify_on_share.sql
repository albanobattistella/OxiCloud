-- ════════════════════════════════════════════════════════════════════════════
-- Per-user opt-out for share-notification emails (PR N1, share-notification
-- pipeline). Pairs with the operator kill switch
-- `OXICLOUD_NOTIFY_INTERNAL_USERS_ON_SHARE` — the env flag affects all
-- internal-user sends; this column scopes the decision to one recipient.
-- ════════════════════════════════════════════════════════════════════════════
-- TRUE = the user wants email when someone shares a resource with them
--        (default for both pre-existing and freshly-created rows).
-- FALSE = the user has unchecked the profile checkbox; the share grant is
--        still created normally, but `RecipientNotificationService` returns
--        `NotApplicable { reason: "recipient_opted_out" }` and no mail is
--        dispatched. The granter sees a clear toast.
--
-- Applies uniformly to the plain-notification arm — the path that fires for
-- internal users, OIDC users, and password users. Magic-link invitations to
-- newly-provisioned external users always send regardless of this column,
-- because the link is the only way the external user can sign in for the
-- first time; suppressing it would lock them out of the share entirely.
-- (Once they have an account and have opted out, subsequent shares from
-- other granters do honor the flag.)
--
-- DEFAULT TRUE matches the pre-PR-N1 behaviour for external users (they
-- always received invitations); for internal users it ships the new
-- "you've been shared a folder" notification turned on by default. A
-- noisier inbox is the trade-off; the checkbox is the safety valve.

ALTER TABLE auth.users
    ADD COLUMN notify_on_share BOOLEAN NOT NULL DEFAULT TRUE;

COMMENT ON COLUMN auth.users.notify_on_share IS
    'Per-user opt-out for share-notification emails. TRUE (default) =
     receive an email when someone grants access to a resource;
     FALSE = grant still created, but no mail is sent
     (RecipientNotificationService returns NotApplicable). The
     operator-level kill switch OXICLOUD_NOTIFY_INTERNAL_USERS_ON_SHARE
     is a separate, broader knob; this column is the per-user fine
     grain. Magic-link first-invitations to externals bypass the
     check — see the column comment for the rationale.';
