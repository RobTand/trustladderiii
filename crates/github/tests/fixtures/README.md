# Webhook fixtures

These are recorded JSON fixtures with fictional accounts, repositories and commits.
They preserve the documented GitHub payload nesting. They were not captured from
the live app and contain no credentials.

The recordings retain only fields used in slice one, plus fields that distinguish
a close from a merge. Unknown fields are accepted. Check completion payloads may
omit a sender. Issue label payloads must retain the sender.

Source: [GitHub webhook events and payloads](https://docs.github.com/en/webhooks/webhook-events-and-payloads).
The issue, comment, pull request, check run and check suite sections define these shapes.
