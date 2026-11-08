# Remove at K5

K5 owns deletion of the adjacent installed pb_nightly.py AND the whole nightly_proof adapter-loading hook in dev_evidence.py in the same guarded unit. Remove the PB index authority. Retain immutable historical evidence outside the gate.

Check the installed gate after deletion, without scanning historical evidence:

```bash
command -v rg >/dev/null || exit 1
if rg -n 'pb_nightly|tl3_pb_nightly|pb-nightly:|PB_NIGHTLY' /home/rob/tmp/tl3-build/dev_evidence.py; then
  exit 1
else
  rg_status=$?
  [ "$rg_status" -eq 1 ] || exit "$rg_status"
fi
[ ! -e /home/rob/tmp/tl3-build/pb_nightly.py ]
```

Both checks must succeed. The K5 owning source unit must separately update the authoritative nightly definition and release gate together. This note does not authorize removal now.
