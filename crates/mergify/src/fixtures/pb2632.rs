//! prismabuild#2632 (2026-11-02): every check run on head 4309359, from
//! `GET /repos/RobTand/prismabuild/commits/{head}/check-runs`, trimmed to the
//! fields TL3 reads. Mergify waits on `scopes-available`; `pr-gate` failed and
//! `gate` was skipped (#447).
pub const PB2632_CHECK_RUNS: &str = r####"[
 {
  "id": 125384984269,
  "name": "regression-test",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "external_id": "ab22b130-38a5-51db-8c22-486262248b6c",
  "status": "completed",
  "conclusion": "failure",
  "completed_at": "2026-11-02T15:59:19Z",
  "app": {
   "id": 15368,
   "slug": "github-actions"
  },
  "output": {
   "title": null,
   "summary": null,
   "text": null
  },
  "check_suite": {
   "id": 112398939323
  }
 },
 {
  "id": 125384037414,
  "name": "gate",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "external_id": "8457ab43-e892-51f7-aa5b-c18568aa0813",
  "status": "completed",
  "conclusion": "skipped",
  "completed_at": "2026-11-02T15:57:31Z",
  "app": {
   "id": 15368,
   "slug": "github-actions"
  },
  "output": {
   "title": null,
   "summary": null,
   "text": null
  },
  "check_suite": {
   "id": 112397949893
  }
 },
 {
  "id": 125384033926,
  "name": "pr-gate",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "external_id": "bd121889-ab83-53e8-a881-b318d9fa7666",
  "status": "completed",
  "conclusion": "failure",
  "completed_at": "2026-11-02T15:57:41Z",
  "app": {
   "id": 15368,
   "slug": "github-actions"
  },
  "output": {
   "title": null,
   "summary": null,
   "text": null
  },
  "check_suite": {
   "id": 112397949893
  }
 },
 {
  "id": 125383705539,
  "name": "regression-test",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "external_id": "ecf14096-5f62-5bae-b988-5d4f45900fdc",
  "status": "completed",
  "conclusion": "failure",
  "completed_at": "2026-11-02T15:57:06Z",
  "app": {
   "id": 15368,
   "slug": "github-actions"
  },
  "output": {
   "title": null,
   "summary": null,
   "text": null
  },
  "check_suite": {
   "id": 112397950184
  }
 },
 {
  "id": 125383705195,
  "name": "Post-merge full run",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "external_id": "41095c81-c110-5203-a363-4dd9601facfa",
  "status": "completed",
  "conclusion": "skipped",
  "completed_at": "2026-11-02T15:56:56Z",
  "app": {
   "id": 15368,
   "slug": "github-actions"
  },
  "output": {
   "title": null,
   "summary": null,
   "text": null
  },
  "check_suite": {
   "id": 112397949893
  }
 },
 {
  "id": 125383704923,
  "name": "Post-merge hosted lane (${{ matrix.runner }})",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "external_id": "3f55b906-64c1-51d4-81b7-b1be8ff953cd",
  "status": "completed",
  "conclusion": "skipped",
  "completed_at": "2026-11-02T15:56:56Z",
  "app": {
   "id": 15368,
   "slug": "github-actions"
  },
  "output": {
   "title": null,
   "summary": null,
   "text": null
  },
  "check_suite": {
   "id": 112397949893
  }
 },
 {
  "id": 125383703204,
  "name": "Bazel",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "external_id": "0efaa06d-fdd0-53ed-af08-c2e69e1f3520",
  "status": "completed",
  "conclusion": "failure",
  "completed_at": "2026-11-02T15:57:18Z",
  "app": {
   "id": 15368,
   "slug": "github-actions"
  },
  "output": {
   "title": null,
   "summary": null,
   "text": null
  },
  "check_suite": {
   "id": 112397949893
  }
 },
 {
  "id": 125383702937,
  "name": "Hosted lane",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "external_id": "81e74875-0fb7-56ea-8913-c43853b955d1",
  "status": "completed",
  "conclusion": "failure",
  "completed_at": "2026-11-02T15:57:31Z",
  "app": {
   "id": 15368,
   "slug": "github-actions"
  },
  "output": {
   "title": null,
   "summary": null,
   "text": null
  },
  "check_suite": {
   "id": 112397949893
  }
 },
 {
  "id": 125383702118,
  "name": "issue-link",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "external_id": "910c4a39-8a6b-5303-8338-97d3eaa7b411",
  "status": "completed",
  "conclusion": "success",
  "completed_at": "2026-11-02T15:57:15Z",
  "app": {
   "id": 15368,
   "slug": "github-actions"
  },
  "output": {
   "title": null,
   "summary": null,
   "text": null
  },
  "check_suite": {
   "id": 112397949989
  }
 },
 {
  "id": 125383697558,
  "name": "Mergify Merge Queue",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "external_id": "engine:2632",
  "status": "completed",
  "conclusion": "neutral",
  "completed_at": "2026-11-02T15:59:53Z",
  "app": {
   "id": 10562,
   "slug": "mergify"
  },
  "output": {
   "title": "Waiting for queue conditions to match",
   "summary": "### Queue entry conditions\n\n- [ ] `scopes-available` [waiting for pull request scopes to be reported]\n- [X] `-closed` [📌 queue requirement]\n- [X] `-conflict` [📌 queue requirement]\n- [X] `-draft` [📌 queue requirement]\n- [X] any of [📌 queue -> configuration change requirements]:\n  - [X] `-mergify-configuration-changed`\n  - [ ] `check-success = @mergify/Configuration changed`\n  - [ ] `check-success = @mergify/Configuration has been deleted`\n- [X] any of [📌 queue requirement]:\n  - [X] `check-neutral = @mergify/Mergify Merge Protections`\n  - [ ] `check-skipped = @mergify/Mergify Merge Protections`\n  - [ ] `check-success = @mergify/Mergify Merge Protections`\n- [X] any of [🔀 queue conditions]:\n  - [X] all of [📌 queue conditions of queue rule `default`]:\n    - [X] `-draft`\n    - [X] `base = main`\n    - [X] [`github-review-approved`](https://docs.mergify.com/configuration/conditions/#pull-request-attributes-github-review-approved) [🛡 GitHub branch protection]\n    - [X] any of:\n      - [X] all of:\n        - [X] `check-success = @tl3-robtand/TL3 approval`\n        - [X] `label = tl3-queue`\n      - [ ] all of:\n        - [ ] `check-success = pr-gate`\n        - [ ] `label = queue`\n    - [X] any of [🛡 GitHub branch protection]:\n      - [X] `check-success = issue-link`\n      - [ ] `check-neutral = issue-link`\n      - [ ] `check-skipped = issue-link`\n  - [ ] all of [📌 queue conditions of queue rule `release`]:\n    - [ ] `check-success = pr-gate`\n    - [ ] `label = queue`\n    - [ ] `label = release`\n    - [X] `-draft`\n    - [X] `base = main`\n    - [X] [`github-review-approved`](https://docs.mergify.com/configuration/conditions/#pull-request-attributes-github-review-approved) [🛡 GitHub branch protection]\n    - [X] any of [🛡 GitHub branch protection]:\n      - [X] `check-success = issue-link`\n      - [ ] `check-neutral = issue-link`\n      - [ ] `check-skipped = issue-link`\n\n---\nYou can queue this pull request now with `@Mergifyio queue`: it will enter the merge queue once these conditions match.\n\n<span itemprop=\"mergify.v1.eyJ2ZXJzaW9uIjoxLCJzdGF0ZSI6IndhaXRpbmciLCJxdWV1ZV9ydWxlX25hbWUiOm51bGwsInF1ZXVlZF9hdCI6bnVsbCwiZXN0aW1hdGVkX3RpbWVfb2ZfbWVyZ2UiOm51bGwsInNwZWN1bGF0aXZlX2NoZWNrX3ByIjpudWxsLCJyZXF1aXJlZF9jb25kaXRpb25zIjpbeyJkZXNjcmlwdGlvbiI6InNjb3Blcy1hdmFpbGFibGUiLCJtYXRjaCI6ZmFsc2UsInN1YmNvbmRpdGlvbnMiOltdLCJub3RlIjoid2FpdGluZyBmb3IgcHVsbCByZXF1ZXN0IHNjb3BlcyB0byBiZSByZXBvcnRlZCJ9LHsiZGVzY3JpcHRpb24iOiItY2xvc2VkIiwibWF0Y2giOnRydWUsInN1YmNvbmRpdGlvbnMiOltdLCJub3RlIjoiXHVkODNkXHVkY2NjIHF1ZXVlIHJlcXVpcmVtZW50In0seyJkZXNjcmlwdGlvbiI6Ii1jb25mbGljdCIsIm1hdGNoIjp0cnVlLCJzdWJjb25kaXRpb25zIjpbXSwibm90ZSI6Ilx1ZDgzZFx1ZGNjYyBxdWV1ZSByZXF1aXJlbWVudCJ9LHsiZGVzY3JpcHRpb24iOiItZHJhZnQiLCJtYXRjaCI6dHJ1ZSwic3ViY29uZGl0aW9ucyI6W10sIm5vdGUiOiJcdWQ4M2RcdWRjY2MgcXVldWUgcmVxdWlyZW1lbnQifSx7ImRlc2NyaXB0aW9uIjoiYW55IG9mIiwibWF0Y2giOnRydWUsInN1YmNvbmRpdGlvbnMiOlt7ImRlc2NyaXB0aW9uIjoiLW1lcmdpZnktY29uZmlndXJhdGlvbi1jaGFuZ2VkIiwibWF0Y2giOnRydWUsInN1YmNvbmRpdGlvbnMiOltdfSx7ImRlc2NyaXB0aW9uIjoiY2hlY2stc3VjY2VzcyA9IEBtZXJnaWZ5L0NvbmZpZ3VyYXRpb24gY2hhbmdlZCIsIm1hdGNoIjpmYWxzZSwic3ViY29uZGl0aW9ucyI6W119LHsiZGVzY3JpcHRpb24iOiJjaGVjay1zdWNjZXNzID0gQG1lcmdpZnkvQ29uZmlndXJhdGlvbiBoYXMgYmVlbiBkZWxldGVkIiwibWF0Y2giOmZhbHNlLCJzdWJjb25kaXRpb25zIjpbXX1dLCJub3RlIjoiXHVkODNkXHVkY2NjIHF1ZXVlIC0-IGNvbmZpZ3VyYXRpb24gY2hhbmdlIHJlcXVpcmVtZW50cyJ9LHsiZGVzY3JpcHRpb24iOiJhbnkgb2YiLCJtYXRjaCI6dHJ1ZSwic3ViY29uZGl0aW9ucyI6W3siZGVzY3JpcHRpb24iOiJjaGVjay1uZXV0cmFsID0gQG1lcmdpZnkvTWVyZ2lmeSBNZXJnZSBQcm90ZWN0aW9ucyIsIm1hdGNoIjp0cnVlLCJzdWJjb25kaXRpb25zIjpbXX0seyJkZXNjcmlwdGlvbiI6ImNoZWNrLXNraXBwZWQgPSBAbWVyZ2lmeS9NZXJnaWZ5IE1lcmdlIFByb3RlY3Rpb25zIiwibWF0Y2giOmZhbHNlLCJzdWJjb25kaXRpb25zIjpbXX0seyJkZXNjcmlwdGlvbiI6ImNoZWNrLXN1Y2Nlc3MgPSBAbWVyZ2lmeS9NZXJnaWZ5IE1lcmdlIFByb3RlY3Rpb25zIiwibWF0Y2giOmZhbHNlLCJzdWJjb25kaXRpb25zIjpbXX1dLCJub3RlIjoiXHVkODNkXHVkY2NjIHF1ZXVlIHJlcXVpcmVtZW50In0seyJkZXNjcmlwdGlvbiI6ImFueSBvZiIsIm1hdGNoIjp0cnVlLCJzdWJjb25kaXRpb25zIjpbeyJkZXNjcmlwdGlvbiI6ImFsbCBvZiIsIm1hdGNoIjp0cnVlLCJzdWJjb25kaXRpb25zIjpbeyJkZXNjcmlwdGlvbiI6Ii1kcmFmdCIsIm1hdGNoIjp0cnVlLCJzdWJjb25kaXRpb25zIjpbXX0seyJkZXNjcmlwdGlvbiI6ImJhc2UgPSBtYWluIiwibWF0Y2giOnRydWUsInN1YmNvbmRpdGlvbnMiOltdfSx7ImRlc2NyaXB0aW9uIjoiZ2l0aHViLXJldmlldy1hcHByb3ZlZCIsIm1hdGNoIjp0cnVlLCJzdWJjb25kaXRpb25zIjpbXSwibm90ZSI6Ilx1ZDgzZFx1ZGVlMSBHaXRIdWIgYnJhbmNoIHByb3RlY3Rpb24ifSx7ImRlc2NyaXB0aW9uIjoiYW55IG9mIiwibWF0Y2giOnRydWUsInN1YmNvbmRpdGlvbnMiOlt7ImRlc2NyaXB0aW9uIjoiYWxsIG9mIiwibWF0Y2giOnRydWUsInN1YmNvbmRpdGlvbnMiOlt7ImRlc2NyaXB0aW9uIjoiY2hlY2stc3VjY2VzcyA9IEB0bDMtcm9idGFuZC9UTDMgYXBwcm92YWwiLCJtYXRjaCI6dHJ1ZSwic3ViY29uZGl0aW9ucyI6W119LHsiZGVzY3JpcHRpb24iOiJsYWJlbCA9IHRsMy1xdWV1ZSIsIm1hdGNoIjp0cnVlLCJzdWJjb25kaXRpb25zIjpbXX1dfSx7ImRlc2NyaXB0aW9uIjoiYWxsIG9mIiwibWF0Y2giOmZhbHNlLCJzdWJjb25kaXRpb25zIjpbeyJkZXNjcmlwdGlvbiI6ImNoZWNrLXN1Y2Nlc3MgPSBwci1nYXRlIiwibWF0Y2giOmZhbHNlLCJzdWJjb25kaXRpb25zIjpbXX0seyJkZXNjcmlwdGlvbiI6ImxhYmVsID0gcXVldWUiLCJtYXRjaCI6ZmFsc2UsInN1YmNvbmRpdGlvbnMiOltdfV19XX0seyJkZXNjcmlwdGlvbiI6ImFueSBvZiIsIm1hdGNoIjp0cnVlLCJzdWJjb25kaXRpb25zIjpbeyJkZXNjcmlwdGlvbiI6ImNoZWNrLXN1Y2Nlc3MgPSBpc3N1ZS1saW5rIiwibWF0Y2giOnRydWUsInN1YmNvbmRpdGlvbnMiOltdfSx7ImRlc2NyaXB0aW9uIjoiY2hlY2stbmV1dHJhbCA9IGlzc3VlLWxpbmsiLCJtYXRjaCI6ZmFsc2UsInN1YmNvbmRpdGlvbnMiOltdfSx7ImRlc2NyaXB0aW9uIjoiY2hlY2stc2tpcHBlZCA9IGlzc3VlLWxpbmsiLCJtYXRjaCI6ZmFsc2UsInN1YmNvbmRpdGlvbnMiOltdfV0sIm5vdGUiOiJcdWQ4M2RcdWRlZTEgR2l0SHViIGJyYW5jaCBwcm90ZWN0aW9uIn1dLCJub3RlIjoiXHVkODNkXHVkY2NjIHF1ZXVlIGNvbmRpdGlvbnMgb2YgcXVldWUgcnVsZSBgZGVmYXVsdGAifSx7ImRlc2NyaXB0aW9uIjoiYWxsIG9mIiwibWF0Y2giOmZhbHNlLCJzdWJjb25kaXRpb25zIjpbeyJkZXNjcmlwdGlvbiI6ImNoZWNrLXN1Y2Nlc3MgPSBwci1nYXRlIiwibWF0Y2giOmZhbHNlLCJzdWJjb25kaXRpb25zIjpbXX0seyJkZXNjcmlwdGlvbiI6ImxhYmVsID0gcXVldWUiLCJtYXRjaCI6ZmFsc2UsInN1YmNvbmRpdGlvbnMiOltdfSx7ImRlc2NyaXB0aW9uIjoibGFiZWwgPSByZWxlYXNlIiwibWF0Y2giOmZhbHNlLCJzdWJjb25kaXRpb25zIjpbXX0seyJkZXNjcmlwdGlvbiI6Ii1kcmFmdCIsIm1hdGNoIjp0cnVlLCJzdWJjb25kaXRpb25zIjpbXX0seyJkZXNjcmlwdGlvbiI6ImJhc2UgPSBtYWluIiwibWF0Y2giOnRydWUsInN1YmNvbmRpdGlvbnMiOltdfSx7ImRlc2NyaXB0aW9uIjoiZ2l0aHViLXJldmlldy1hcHByb3ZlZCIsIm1hdGNoIjp0cnVlLCJzdWJjb25kaXRpb25zIjpbXSwibm90ZSI6Ilx1ZDgzZFx1ZGVlMSBHaXRIdWIgYnJhbmNoIHByb3RlY3Rpb24ifSx7ImRlc2NyaXB0aW9uIjoiYW55IG9mIiwibWF0Y2giOnRydWUsInN1YmNvbmRpdGlvbnMiOlt7ImRlc2NyaXB0aW9uIjoiY2hlY2stc3VjY2VzcyA9IGlzc3VlLWxpbmsiLCJtYXRjaCI6dHJ1ZSwic3ViY29uZGl0aW9ucyI6W119LHsiZGVzY3JpcHRpb24iOiJjaGVjay1uZXV0cmFsID0gaXNzdWUtbGluayIsIm1hdGNoIjpmYWxzZSwic3ViY29uZGl0aW9ucyI6W119LHsiZGVzY3JpcHRpb24iOiJjaGVjay1za2lwcGVkID0gaXNzdWUtbGluayIsIm1hdGNoIjpmYWxzZSwic3ViY29uZGl0aW9ucyI6W119XSwibm90ZSI6Ilx1ZDgzZFx1ZGVlMSBHaXRIdWIgYnJhbmNoIHByb3RlY3Rpb24ifV0sIm5vdGUiOiJcdWQ4M2RcdWRjY2MgcXVldWUgY29uZGl0aW9ucyBvZiBxdWV1ZSBydWxlIGByZWxlYXNlYCJ9XSwibm90ZSI6Ilx1ZDgzZFx1ZGQwMCBxdWV1ZSBjb25kaXRpb25zIn1dfQ\"></span>",
   "text": null
  },
  "check_suite": {
   "id": 112397907964
  }
 },
 {
  "id": 125383691915,
  "name": "Mergify Merge Protections",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "external_id": "2632",
  "status": "completed",
  "conclusion": "neutral",
  "completed_at": "2026-11-02T15:57:25Z",
  "app": {
   "id": 10562,
   "slug": "mergify"
  },
  "output": {
   "title": "No merge protections matched",
   "summary": "This pull request does not match any conditions of the merge protections.",
   "text": null
  },
  "check_suite": {
   "id": 112397907964
  }
 },
 {
  "id": 125383686087,
  "name": "Summary",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "external_id": "2632",
  "status": "completed",
  "conclusion": "success",
  "completed_at": "2026-11-02T15:59:51Z",
  "app": {
   "id": 10562,
   "slug": "mergify"
  },
  "output": {
   "title": "2 potential rules",
   "summary": "### Rule: queue pull requests labelled \\\"queue\\\" (queue)\n- [ ] `label = queue`\n- [ ] `scopes-available` [waiting for pull request scopes to be reported]\n- [X] `-closed` [📌 queue requirement]\n- [X] `-conflict` [📌 queue requirement]\n- [X] `-draft`\n- [X] `-draft` [📌 queue requirement]\n- [X] `base = main`\n- [X] any of: [📌 queue -> configuration change requirements]\n  - [X] `-mergify-configuration-changed`\n  - [ ] `check-success = @mergify/Configuration changed`\n  - [ ] `check-success = @mergify/Configuration has been deleted`\n- [X] any of: [📌 queue requirement]\n  - [X] `check-neutral = @mergify/Mergify Merge Protections`\n  - [ ] `check-skipped = @mergify/Mergify Merge Protections`\n  - [ ] `check-success = @mergify/Mergify Merge Protections`\n- [X] any of: [🔀 queue conditions]\n  - [X] all of: [📌 queue conditions of queue rule `default`]\n    - [X] `-draft`\n    - [X] `base = main`\n    - [X] [`github-review-approved`](https://docs.mergify.com/configuration/conditions/#pull-request-attributes-github-review-approved) [🛡 GitHub branch protection]\n    - [X] any of:\n      - [X] all of:\n        - [X] `check-success = @tl3-robtand/TL3 approval`\n        - [X] `label = tl3-queue`\n      - [ ] all of:\n        - [ ] `check-success = pr-gate`\n        - [ ] `label = queue`\n    - [X] any of: [🛡 GitHub branch protection]\n      - [X] `check-success = issue-link`\n      - [ ] `check-neutral = issue-link`\n      - [ ] `check-skipped = issue-link`\n  - [ ] all of: [📌 queue conditions of queue rule `release`]\n    - [ ] `check-success = pr-gate`\n    - [ ] `label = queue`\n    - [ ] `label = release`\n    - [X] `-draft`\n    - [X] `base = main`\n    - [X] [`github-review-approved`](https://docs.mergify.com/configuration/conditions/#pull-request-attributes-github-review-approved) [🛡 GitHub branch protection]\n    - [X] any of: [🛡 GitHub branch protection]\n      - [X] `check-success = issue-link`\n      - [ ] `check-neutral = issue-link`\n      - [ ] `check-skipped = issue-link`\n\n### Rule: Queue TL3\\-approved PRs (queue)\n- [ ] `scopes-available` [waiting for pull request scopes to be reported]\n- [X] `-closed` [📌 queue requirement]\n- [X] `-conflict` [📌 queue requirement]\n- [X] `-draft`\n- [X] `-draft` [📌 queue requirement]\n- [X] `base = main`\n- [X] `check-success = @tl3-robtand/TL3 approval`\n- [X] `label = tl3-queue`\n- [X] any of: [📌 queue -> configuration change requirements]\n  - [X] `-mergify-configuration-changed`\n  - [ ] `check-success = @mergify/Configuration changed`\n  - [ ] `check-success = @mergify/Configuration has been deleted`\n- [X] any of: [📌 queue requirement]\n  - [X] `check-neutral = @mergify/Mergify Merge Protections`\n  - [ ] `check-skipped = @mergify/Mergify Merge Protections`\n  - [ ] `check-success = @mergify/Mergify Merge Protections`\n- [X] any of: [🔀 queue conditions]\n  - [X] all of: [📌 queue conditions of queue rule `default`]\n    - [X] `-draft`\n    - [X] `base = main`\n    - [X] [`github-review-approved`](https://docs.mergify.com/configuration/conditions/#pull-request-attributes-github-review-approved) [🛡 GitHub branch protection]\n    - [X] any of:\n      - [X] all of:\n        - [X] `check-success = @tl3-robtand/TL3 approval`\n        - [X] `label = tl3-queue`\n      - [ ] all of:\n        - [ ] `check-success = pr-gate`\n        - [ ] `label = queue`\n    - [X] any of: [🛡 GitHub branch protection]\n      - [X] `check-success = issue-link`\n      - [ ] `check-neutral = issue-link`\n      - [ ] `check-skipped = issue-link`\n\n<hr />\n\n<details>\n<summary>Mergify commands and options</summary>\n\n<br />\n\nMore conditions and actions can be found in the [documentation](https://docs.mergify.com/).\n\nYou can also trigger Mergify actions by commenting on this pull request:\n\n- `@Mergifyio refresh` will re-evaluate the rules\n- `@Mergifyio rebase` will rebase this PR on its base branch\n- `@Mergifyio update` will merge the base branch into this PR\n- `@Mergifyio backport <destination>` will backport this PR on `<destination>` branch\n\nAdditionally, on Mergify [dashboard](https://dashboard.mergify.com) you can:\n\n- look at your merge queues\n- generate the Mergify configuration with the config editor.\n\nFinally, you can contact us on https://mergify.com\n</details>\n",
   "text": null
  },
  "check_suite": {
   "id": 112397907964
  }
 },
 {
  "id": 125383635497,
  "name": "TL3 approval",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "external_id": "effect:97:8:4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "status": "completed",
  "conclusion": "success",
  "completed_at": "2026-11-02T15:56:49Z",
  "app": {
   "id": 5542171,
   "slug": "tl3-robtand"
  },
  "output": {
   "title": null,
   "summary": null,
   "text": null
  },
  "check_suite": {
   "id": 112397903989
  }
 }
]"####;
/// prismabuild#2632 itself, from `GET /repos/RobTand/prismabuild/pulls/2632`, trimmed.
pub const PB2632_PULL_REQUEST: &str = r####"{
 "title": "[P2] Add a stdlib-only gate GPU lease client",
 "body": "Refs #2630.\n\n<!-- tl3:effect:6566666563743a39373a39 -->",
 "labels": [
  {
   "name": "tl3-queue"
  }
 ],
 "number": 2632,
 "user": {
  "id": 350304049,
  "login": "tl3-robtand[bot]"
 },
 "state": "open",
 "merged_at": null,
 "merge_commit_sha": "bd86d16e321a413e9be3091e198c896c7e085abc",
 "mergeable": true,
 "mergeable_state": "unstable",
 "head": {
  "ref": "prismabuild-2630-tl3-97",
  "sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "repo": null
 },
 "base": {
  "ref": "main",
  "sha": "071094caa54922143398eb0397acb61bf3fed9f9",
  "repo": null
 },
 "updated_at": "2026-11-02T15:59:07Z"
}"####;
/// prismabuild#2632's workflow runs, from `GET /repos/RobTand/prismabuild/actions/runs?head_sha=4309359…`.
/// `gate.yml` triggers on `opened`, `synchronize` and `reopened` only; `regression-test.yml`
/// (`pull_request_target`) also on `edited`, so only the gate needs a rerun after a body edit.
pub const PB2632_WORKFLOW_RUNS: &str = r####"[
 {
  "id": 41476965585,
  "name": "Regression test",
  "path": ".github/workflows/regression-test.yml",
  "event": "pull_request_target",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "run_attempt": 1,
  "status": "completed",
  "conclusion": "failure",
  "check_suite_id": 112398939323
 },
 {
  "id": 41476603984,
  "name": "Regression test",
  "path": ".github/workflows/regression-test.yml",
  "event": "pull_request_target",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "run_attempt": 1,
  "status": "completed",
  "conclusion": "failure",
  "check_suite_id": 112397950184
 },
 {
  "id": 41476603917,
  "name": "Issue link",
  "path": ".github/workflows/issue-link.yml",
  "event": "pull_request",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "run_attempt": 1,
  "status": "completed",
  "conclusion": "success",
  "check_suite_id": 112397949989
 },
 {
  "id": 41476603888,
  "name": "Merge gate",
  "path": ".github/workflows/gate.yml",
  "event": "pull_request",
  "head_sha": "4309359678fc08ddfb44f12439e3a3744ee3eb11",
  "run_attempt": 1,
  "status": "completed",
  "conclusion": "failure",
  "check_suite_id": 112397949893
 }
]"####;
