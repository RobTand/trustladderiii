"""P4 pre-extraction run/_run, verbatim from bff1b285. Test only."""
# ruff: noqa: F821

def run(request, emit: Callable[[dict], None]):
    from tl3_workers.tracing import observe, record_reply

    from tl3_workers.facts import observe as observe_facts

    with observe_facts(request), observe(request):
        summary = Summary(request)
        token = current.set(summary)
        prompt = run_prompt.set(request.get("prompt", {}).get("text"))
        test_cpus = run_test_cpus.set(request.get("test_cpus", 1))
        summary.save()
        reply, error = None, None
        try:
            reply = _run(request, emit)
            record_reply(reply)
        except BaseException as exc:
            error = exc
            raise
        finally:
            summary.finish(reply, error)
            run_prompt.reset(prompt)
            run_test_cpus.reset(test_cpus)
            current.reset(token)

def _run(request, emit):
    role = ROLES[request["role"]]
    model = create_model(request)
    if request["inputs"].get("memories"):
        request = {**request, "inputs": {**request["inputs"],
                                         "memories": redacted_memories(request["inputs"]["memories"])}}
    if role.prepare_inputs is not None:
        request = {**request, "inputs": role.prepare_inputs(request["inputs"], request)}
    forecast = request["inputs"].get("forecast", {})
    with Worktree(request["worktree"], generated_files=forecast.get("generated_files", ()),
                  generator=forecast.get("generator")) if role.tools else nullcontext() as worktree:
        if worktree is not None:
            worktree.environment_errors_end_run = role.environment_errors_end_run
            worktree.hindsight_key_file = request.get("hindsight_key_file")
            worktree.progress_emit = emit
        if role.name == 'planner':
            worktree.forcing_rehearsals = ForcingRehearsals(
                worktree, request['inputs'], checkout=request['worktree'], checkpoint=request.get('session_file'))
        if role.run is None:
            control = TurnControl()
            agent = build_agent(model, role, worktree, context_size=request["inputs"].get(
                "context_size", request["inputs"].get("forecast", {}).get("context_tokens", 150_000)),
                control=control)
            model_inputs = request["inputs"]
            if role.name in {"ingest", "audit", "planner"}:
                model_inputs = {"forcing_capabilities": forcing_capabilities(run_test_cpus.get()), **model_inputs}
            if role.name == "planner":
                maker_budget = ROLES["maker"].exploration_budget
                model_inputs = {**model_inputs, "maker_budget": {
                    "tool_turns": maker_budget.tool_turns, "input_tokens": maker_budget.input_tokens,
                    "finalization_turns": maker_budget.finalization_turns,
                    "no_progress_window": maker_budget.no_progress_window}}
            ledger = getattr(worktree, 'forcing_rehearsals', None)
            if ledger is not None and (ledger.candidates or ledger.active):
                model_inputs = {**model_inputs, 'forcing_rehearsal_state': {
                    'candidates': ledger.candidates, 'pending': ledger.active,
                    'remaining_candidates': 8 - ledger.attempts}}
            history = [initial_message(request, model_inputs)]
            draft_error = None

            def checked(text):
                nonlocal draft_error
                if ledger is not None:
                    ledger.raise_infrastructure()
                if ledger is not None and ledger.failure:
                    return _checked(role, request['inputs'], {
                        'kind': 'cant_meet', 'files': [], 'evidence_contract': [], 'report': ledger.failure['message']}, worktree=worktree)
                try:
                    result = _checked(role, request["inputs"], read_reply(text), worktree=worktree)
                except PlanConsistencyError as error:
                    if ledger is not None and any(item['status'] == 'refused' for item in ledger.candidates):
                        return refused_final()
                    if draft_error is None:
                        draft_error = error
                    raise
                except (ValueError, ValidationError):
                    if ledger is not None and any(item['status'] == 'refused' for item in ledger.candidates):
                        return refused_final()
                    raise
                if draft_error is not None:
                    draft_error.constrain(result)
                return result

            def refused_final():
                rejected = next(item for item in reversed(ledger.candidates) if item['status'] == 'refused')
                return _checked(role, request['inputs'], {
                    'kind': 'cant_meet', 'files': [], 'evidence_contract': [],
                    'report': rejected['message']}, worktree=worktree)

            reply = repaired(control, history, lambda: _run_agent(agent, history, emit),
                             checked)
        else:
            reply = role.run(model, request, worktree, emit)
            if not role.validates_reply:
                _checked(role, request["inputs"], reply)
    emit({"event": "reply", "role": role.name, "reply": reply})
    return reply
