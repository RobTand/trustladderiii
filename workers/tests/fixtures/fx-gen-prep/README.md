# FX-GEN-PREP typed generator preparation failure

`preparation-failure-artifact.json` is an artifact-only copy, byte for byte
(922 bytes, no trailing newline, sha256
b37f9a34d949c72d397fa08fa778860cd8e8a38c1fafb505c5658a310439217d), of record
`full/generator-prep-failure` `worker-artifact.json` from:

- FX-GEN-PREP candidate source 80ddef3e97482f42e56b5610588b516ad50c73ef,
  tree 8ce7425e6062e7da1667b7d59363df77f1188e6f;
- PB action aeddee77bea0157e658731de27e756a8f0e2429898f7b1d6455966d7c931bc13.
  That run's OVERALL verdict is FAIL. Only its typed-FAILED real-closure check
  passed.

Sibling records of that run, not copied: receipt.json, report.json,
request.json, intent.json, state.json and eventlog.jsonl.

The tests use only the artifact. Their RunBinding is the FluxFake's synthetic
`worker:aaaa…` binding. It is separate from that run's receipt and report
digests, and it is not live identity proof. Negative cases change only an
in-memory deep copy.

Do not trim or reserialize this file. Replace it only from a final FX artifact,
never by hand.
