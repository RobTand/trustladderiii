# Webhook fixtures

These are recorded JSON fixtures with fictional accounts, repositories and commits.
They preserve the documented GitHub payload nesting. They were not captured from
the live app and contain no credentials.

The recordings retain only fields used in slice one, plus fields that distinguish
a close from a merge. Unknown fields are accepted. Check completion payloads may
omit a sender. Issue label payloads must retain the sender.

Source: [GitHub webhook events and payloads](https://docs.github.com/en/webhooks/webhook-events-and-payloads).
The issue, comment, pull request, check run and check suite sections define these shapes.

`pull-request-unlabeled.json` and `merge-group-checks-requested.json` are synthetic
variants for queue ingress. The merge-group payload describes a notification,
not proof that a PR merged.

`actions-check-null-title.json` records the GitHub Actions `Sandbox tests` check
on `RobTand/tl3-sandbox` PR #16, head
`79ac4b99b56123c04c856e2be1976ba25f499f81`, read through REST on 2026-10-30.
Only the client model's fields remain. GitHub returns a non-null `output` object
whose `title` is null. That response blocked TL3 approval reconciliation before
the client accepted nullable titles.
