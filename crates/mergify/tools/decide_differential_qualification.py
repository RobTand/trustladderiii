"""#743 M1b: show each production rewrite's reference test rejects a real mutant."""
import decide_qualification

adapter = decide_qualification.safety_qualification
adapter.CASES = {
    name: (*adapter.CASES[name][:3], '//crates/mergify:test_test_decide_differential')
    for name in ['suites_scan', 'merge_wait', 'facts_outcome', 'person_merge']
}

if __name__ == '__main__':
    adapter.main()
