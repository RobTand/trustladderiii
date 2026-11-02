# TL3 merged-PR graph replay

Old rules fail 23/30; revised rules fail 2/30.

Old core comes from TL3 `81fe2fec027f2f91aa433a10e8206904e38d86c2`. Revised core: `RobTand/prismaquant@fb6555514e2f43727ab73d056f82d8ad34ab64e3:tools/ci/graph_impact.py`.

Each range compares immutable base and head graphs. New rules use package base ceilings and typed edge differences. The JSON artifact retains every full diagnostic and edge.

| PR | Old failure reasons | Revised failure reasons |
|---|---|---|
| #314 | new test_to_test edge: //workers/tests:test_turns -> //workers/tests:test_maker<br>new test_to_test edge: //workers/tests:test_turns -> //workers/tests:test_prompt_cache<br>new test_to_test edge: //workers/tests:test_turns -> //workers/tests:test_reviewer<br>new test_to_test edge: //workers/tests:test_turns -> //workers/tests:test_workers | new test_to_test edge: //workers/tests:test_turns -> //workers/tests:test_maker<br>new test_to_test edge: //workers/tests:test_turns -> //workers/tests:test_prompt_cache<br>new test_to_test edge: //workers/tests:test_turns -> //workers/tests:test_reviewer<br>new test_to_test edge: //workers/tests:test_turns -> //workers/tests:test_workers |
| #347 | 28 tests reach 166, 167 above global budget 164<br>loc_u_pct rose from 30.3874 to 31.818<br>u_pct rose from 30.8718 to 31.03 | pass |
| #364 | 2 tests reach 167 above global budget 166 | pass |
| #350 | 2 tests reach 167 above global budget 166 | pass |
| #360 | u_pct fell from 30.9808 to 30.8394; gain unrecorded | pass |
| #352 | loc_u_pct fell from 31.7106 to 31.5567; gain unrecorded<br>new test_to_test edge: //workers/tests:test_large_files -> //workers/tests:test_maker<br>new test_to_test edge: //workers/tests:test_large_files -> //workers/tests:test_planner<br>new test_to_test edge: //workers/tests:test_large_files -> //workers/tests:test_workers | new test_to_test edge: //workers/tests:test_large_files -> //workers/tests:test_maker<br>new test_to_test edge: //workers/tests:test_large_files -> //workers/tests:test_planner<br>new test_to_test edge: //workers/tests:test_large_files -> //workers/tests:test_workers |
| #368 | 2 tests reach 167 above global budget 166 | pass |
| #365 | 28 tests reach 167, 168 above global budget 166 | pass |
| #379 | pass | pass |
| #375 | 2 tests reach 168 above global budget 167<br>loc_u_pct rose from 31.5606 to 32.0614<br>u_pct rose from 30.7448 to 31.1611 | pass |
| #382 | 2 tests reach 168 above global budget 167 | pass |
| #386 | pass | pass |
| #387 | 2 tests reach 168 above global budget 167<br>loc_u_pct fell from 32.0613 to 30.5988; gain unrecorded<br>u_pct fell from 31.1611 to 31.0368; gain unrecorded | pass |
| #377 | 30 tests reach 169, 170 above global budget 167<br>loc_u_pct rose from 30.5988 to 31.1022<br>u_pct rose from 31.0368 to 31.4774 | pass |
| #384 | 2 tests reach 170 above global budget 169<br>loc_u_pct fell from 31.1022 to 31.0015; gain unrecorded | pass |
| #396 | loc_u_pct fell from 31.0015 to 30.8265; gain unrecorded | pass |
| #397 | 2 tests reach 170 above global budget 169<br>loc_u_pct fell from 30.8265 to 30.6212; gain unrecorded<br>u_pct fell from 31.3776 to 31.1774; gain unrecorded | pass |
| #381 | 30 tests reach 171, 172 above global budget 169 | pass |
| #399 | 2 tests reach 172 above global budget 171 | pass |
| #404 | pass | pass |
| #405 | 2 tests reach 172 above global budget 171 | pass |
| #411 | pass | pass |
| #412 | pass | pass |
| #410 | 2 tests reach 172 above global budget 171 | pass |
| #409 | 30 tests reach 172, 173 above global budget 171<br>loc_u_pct fell from 30.6652 to 30.5183; gain unrecorded | pass |
| #419 | pass | pass |
| #428 | loc_u_pct fell from 30.5363 to 30.3453; gain unrecorded<br>u_pct fell from 31.1482 to 30.9565; gain unrecorded | pass |
| #420 | 31 tests reach 173, 174 above global budget 172<br>loc_u_pct rose from 30.3453 to 30.8454<br>u_pct rose from 30.9565 to 31.3902 | pass |
| #435 | pass | pass |
| #437 | 2 tests reach 174 above global budget 173<br>loc_u_pct rose from 30.8717 to 31.3662<br>u_pct rose from 31.3902 to 31.8283 | pass |
