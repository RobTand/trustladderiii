# CircleCI machine runners

See [the shadow rollout](../../docs/circleci.md) for capacity, coverage, costs,
installation, comparison, and cutover requirements.

The installer builds a separate image from the existing native TL3 runner image.
It does not change GitHub runner files or enable a service. Every slot uses a
private rootless container. The task agent drops privileges before executing job
steps. Store each host's resource token in its mode-600 runner config with
`write_config.py`, piping the token API response through stdin. Never print that
response. The installer and repository contain no credentials.

`evidence/platform.json` records the initial class IDs and slot split. The
initial `evidence/shadow.json` has zero qualifying batches. It is not cutover
evidence. Refresh it with `tools/ci/compare_shadow.py` after the shadow PR merges.
