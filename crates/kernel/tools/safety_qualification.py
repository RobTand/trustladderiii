"""#771 fault injection, invoked only on a disposable BuildBuddy runner.

This adapter runs the normal required Rust proof targets against real-source
mutants. A frontend error or timeout is never accepted as mutant rejection.
"""
import argparse
import json
import os
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'tools/ci'))
import bazel_gate  # noqa: E402
import remote_gate  # noqa: E402


CASES = {
    'retire_upstream_stalled': ('crates/kernel/src/facts.rs', '            m.run_incidents = (m.run_incidents + 1).min(2);', '            m.run_incidents = m.run_incidents.min(2);', '//crates/kernel:verus_facts_transition'),
    'retire_outside_push': ('crates/kernel/src/facts.rs', 'update_recheck_head(&mut w.nodes[i], head);', 'let _ = head;', '//crates/kernel:verus_facts_transition'),
    'retire_ci_failed': ('crates/kernel/src/facts.rs', '        failed_round(w, i, ReviewStop::CiFailed, out);', '        failed_round(w, i, ReviewStop::ChecksFailed, out);', '//crates/kernel:verus_facts_ci_failed'),
    'retire_publishing_ci_tests': ('crates/kernel/src/facts.rs', '    let before_enqueue =\n        m.phase == Phase::Publishing && m.current() == Some(Item::Do(EffectKind::Enqueue));', '    let before_enqueue = false;', '//crates/kernel:verus_facts_ci_failed'),
    'retire_new_request': ('crates/kernel/src/events.rs', 'Event::NewRequest { repo, priority } => {\n            if match repo.index() { Some(r) => r < w.gates.len(), None => false } {\n                let mut node = Node::new(repo);\n                node.priority = priority;', 'Event::NewRequest { repo, priority } => {\n            if match repo.index() { Some(r) => r < w.gates.len(), None => false } {\n                let mut node = Node::new(repo);\n                node.priority = Priority::Unranked;', '//crates/kernel:verus_events'),
    'retire_new_adoption': ('crates/kernel/src/events.rs', '                node.proposed = Some(head);', '                node.proposed = None;', '//crates/kernel:verus_events'),
    'retire_gate_grant_w2': ('crates/kernel/src/ranks.rs', 'm.holds = true;', 'm.holds = false;', '//crates/kernel:verus_ranks_advance_slot'),
    'retire_gate_grant_w3': ('crates/kernel/src/ranks.rs', 'set_phase(m, r.phase());', 'let _ = r;', '//crates/kernel:verus_ranks_advance_slot'),
    'plan_check_rejected_asks': (
        'crates/kernel/src/plan_check.rs',
        'Reply::PlanCheckRejected => {\n            stop_for_review(&mut w.nodes[i], ReviewStop::PlanCantMeet);',
        'Reply::PlanCheckRejected => {\n            ask_rob(w, i, QKind::CantMeet, Cont::Nowhere, 0, _out);',
        '//crates/kernel:verus_plan_check'),
    'plan_check_needs_split_asks': (
        'crates/kernel/src/plan_check.rs',
        'Reply::PlanCheckNeedsSplit => {\n            stop_for_review(&mut w.nodes[i], ReviewStop::PlanCantMeet);',
        'Reply::PlanCheckNeedsSplit => {\n            ask_rob(w, i, QKind::NeedsSplitting, Cont::Nowhere, 0, _out);',
        '//crates/kernel:verus_plan_check'),
    'selector_inventory': ('crates/kernel/src/allocation.rs',
                           'use super::*;',
                           'use super::*;\nkernel_fn! { fn selector_inventory_unlisted() '
                           '-> (result: bool) [ ensures result, ] { true } }',
                           '//crates/kernel:test_test_proof_map'),
    'retire_merge_stopping': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if !matches!(m.phase, Phase::Stopping(_)) { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_closed': ('crates/kernel/src/facts.rs',
                           'm.merged = Some(head);\n            m.phase = Phase::Closed(Outcome::Merged);',
                           'm.merged = None;\n            m.phase = Phase::Closed(Outcome::Merged);',
                           '//crates/kernel:verus_facts_merge'),
    'retire_priority': ('crates/kernel/src/facts.rs',
                        'w.nodes[i].priority = priority;', 'let _ = priority;',
                        '//crates/kernel:verus_facts_transition'),
    'retire_pr_closed': ('crates/kernel/src/facts.rs',
                         'm.pr_open = false;\n            if !m.phase.is_stopping()',
                         'let _ = m.pr_open;\n            if !m.phase.is_stopping()',
                         '//crates/kernel:verus_facts_transition'),
    'retire_conflict': ('crates/kernel/src/facts.rs',
                        'set_phase(m, Phase::Rebasing);', 'let _ = m;',
                        '//crates/kernel:verus_facts_transition'),
    'retire_merge_reviewing': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if m.phase != Phase::Reviewing { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_publishing': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if m.phase != Phase::Publishing { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_queued': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if m.phase != Phase::Queued { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_rebasing': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if m.phase != Phase::Rebasing { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_awaiting_rob': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if m.phase != Phase::AwaitingRob { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_unplanned': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if !matches!(m.phase, Phase::Unplanned) { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_planning': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if !matches!(m.phase, Phase::Planning { .. }) { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_ready': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if !matches!(m.phase, Phase::Ready(_)) { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_making': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if !matches!(m.phase, Phase::Making) { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_checking': ('crates/kernel/src/facts.rs',
                             'm.merged = Some(head);\n    m.pr_open = false;',
                             'if !matches!(m.phase, Phase::Checking) { m.merged = Some(head); }\n    m.pr_open = false;',
                             '//crates/kernel:verus_facts_merge_open'),
    'retire_incident': ('crates/kernel/src/facts.rs',
                        'm.incident_needs_rob = false;',
                        'let _ = m.incident_needs_rob;',
                        '//crates/kernel:verus_facts_incident_cleared'),
    'retire_dequeued': ('crates/kernel/src/facts.rs',
                        'm.paused = true;', 'm.paused = false;',
                        '//crates/kernel:verus_facts_pr_dequeued'),
    'retire_infrastructure': ('crates/kernel/src/facts.rs',
                              'm.run_failures += 1;', 'm.run_failures += 0;',
                              '//crates/kernel:verus_facts_infrastructure_retry'),
    'retire_merge_ingesting': ('crates/kernel/src/facts.rs',
                               'm.merged = Some(head);\n    m.pr_open = false;',
                               'if m.phase != Phase::Ingesting { m.merged = Some(head); }\n    m.pr_open = false;',
                               '//crates/kernel:verus_facts_merge_open'),
    'retire_merge_auditing': ('crates/kernel/src/facts.rs',
                              'm.merged = Some(head);\n    m.pr_open = false;',
                              'if m.phase != Phase::Auditing { m.merged = Some(head); }\n    m.pr_open = false;',
                              '//crates/kernel:verus_facts_merge_open'),
    'witness_skipped': ('crates/kernel/src/witness.rs',
                        'kernel_while! { (i < self.nodes.len()) [',
                        'kernel_while! { (false) [',
                        '//crates/kernel:verus_witness'),
    'resume_stalled': ('crates/kernel/src/commands.rs',
                       '            w.nodes[i].paused = false;',
                       '            w.nodes[i].paused = true;',
                       '//crates/kernel:verus_commands'),
    'reply_stalled': ('crates/kernel/src/work_replies.rs',
                      'set_phase(m, Phase::Checking);',
                      'set_phase(m, Phase::Making);',
                      '//crates/kernel:verus_work_replies'),
    'ack_retained': ('crates/kernel/src/acknowledgements.rs',
                     '    w.nodes[i].effect = None;',
                     '    let _ = e;',
                     '//crates/kernel:verus_acknowledgements_on_ack'),
    'delivery_retained': ('crates/kernel/src/dependencies.rs',
                          'Some(index) => self.dependencies[index] = dependency,',
                          'Some(index) => self.dependencies[index] = Dependency { '
                          'status: DependencyStatus::Pending, ..dependency },',
                          '//crates/kernel:verus_dependencies'),
    'delivery_dispatch': ('crates/kernel/src/facts.rs',
                          'Fact::Dependency { target, status } => w.dependency_fact(i, target, status, out),',
                          'Fact::Dependency { target, status } => { let _ = (target, status); },',
                          '//crates/kernel:verus_facts'),
    'apply_enforcement': ('crates/controller/src/input_errors.rs',
                          'world.counter_exhaustion()',
                          'world.counter_exhaustion().filter(|_| false)',
                          '//crates/controller:test_test_input_error_followups'),
    'translate_enforcement': ('crates/controller/src/lib.rs',
                              'kernel_boundary::validate_raw_event(event).map_err(Error::Invalid)?',
                              'event',
                              '//crates/controller:test_test_input_error_followups'),
    'headroom': ('crates/kernel/src/headroom.rs',
                 'node.generation > u64::MAX - GENERATION_STEP_BUDGET',
                 'node.generation > u64::MAX - (GENERATION_STEP_BUDGET - 1)',
                 '//crates/kernel:verus_kernel_node_counter_exhaustion'),
    'step_bound': ('crates/kernel/src/allocation.rs',
                   '    w.next_question += 1;\n    let m = &mut w.nodes[i];',
                   '    w.next_question += 2;\n    let m = &mut w.nodes[i];',
                   '//crates/kernel:verus_allocation'),
    'command_id': ('crates/kernel/src/lib.rs',
                   'self.last_cmd.checked_add(1)', 'self.last_cmd.checked_add(0)',
                   '//crates/kernel:verus_kernel_node_next_command_id'),
    'command_order': ('crates/kernel/src/commands.rs',
                      'if id <= w.nodes[i].last_cmd {', 'if id == w.nodes[i].last_cmd {',
                      '//crates/kernel:verus_commands'),
    'raw_command': ('crates/controller/src/kernel_boundary.rs',
                    'Err("commands must come from the steering constructors, which assign next_command_id")',
                    'Ok(event)',
                    '//crates/kernel:verus_controller_boundary'),
    'steering_id': ('crates/controller/src/kernel_boundary.rs',
                    'request.next_command_id()', 'Some(request.last_cmd)',
                    '//crates/kernel:verus_controller_boundary'),
    'gate_classification': ('crates/kernel/src/witness.rs',
                            'Phase::Making => Reply::MakerDone {\n                            head: n.head.unwrap_or(HeadId(0)),',
                            'Phase::Making => Reply::MakerDone {\n                            head: HeadId(7),',
                            '//crates/kernel:verus_witness'),
    'gate_waiting': ('crates/kernel/src/checking.rs',
                     'if advance(&mut w, i, &mut out) { return false; }',
                     'if advance(&mut w, i, &mut out) { return true; }',
                     '//crates/kernel:verus_checking_is_settled'),
    'gate_permutation': ('crates/kernel/verus/progress_gate_permutation.rs',
                         'w.nodes[actor] = first;', 'w.nodes[actor] = selected;',
                         '//crates/kernel:verus_progress_gate_permutation'),
    'gate_maker': ('crates/kernel/src/work_replies.rs',
                   'm.head = Some(head);\n            m.checked = None;\n            set_phase(m, Phase::Checking);',
                   'm.head = Some(head);\n            m.checked = Some(head);\n            set_phase(m, Phase::Checking);',
                   '//crates/kernel:verus_work_replies'),
    'gate_permutation_production': ('crates/kernel/src/checking.rs',
                                    'self.check_node(&self.nodes[i])?;',
                                    'self.check_node(&self.nodes[i])?;\n'
                                    '            if i == 0 && self.nodes[i].phase == Phase::Making { return Err(Broken::Gate); }',
                                    '//crates/kernel:verus_checking'),
    'gate_reply_admission': ('crates/kernel/src/reply_admission.rs',
                             'end_operation(&mut w.nodes[i]);',
                             'end_operation(&mut w.nodes[i]);\n    w.nodes[i].pool = true;',
                             '//crates/kernel:verus_reply_admission_on_reply'),

    'gate_apply_merge': ('crates/kernel/src/facts.rs',
                         'm.merged = Some(head);\n    m.pr_open = false;',
                         'if m.phase != Phase::Queued { m.merged = Some(head); }\n    m.pr_open = false;',
                         '//crates/kernel:verus_facts_merge_open'),
    'gate_apply_resume': ('crates/kernel/src/commands.rs',
                          '            w.nodes[i].paused = false;',
                          '            w.nodes[i].paused = true;',
                          '//crates/kernel:verus_commands'),
    'phase_measure_node_sum': ('crates/kernel/src/lib.rs',
                               'total += self.nodes[i].distance();',
                               'total += 0 * self.nodes[i].distance();',
                               '//crates/kernel:verus_kernel_world_measure'),
    'phase_measure_dependency': ('crates/kernel/src/lib.rs',
                                 'if d.status != DependencyStatus::Delivered\n'
                                 '                && !self.nodes[d.node.0 as usize].phase.is_closed()',
                                 'if d.status == DependencyStatus::Delivered\n'
                                 '                && !self.nodes[d.node.0 as usize].phase.is_closed()',
                                 '//crates/kernel:verus_kernel_world_measure'),
    'phase_scope_checker': ('crates/kernel/src/checking.rs',
                            'if self.gates.is_empty() || self.pool_capacity == 0 {',
                            'if self.gates.is_empty() {',
                            '//crates/kernel:verus_checking'),
    'phase_settled_probe': ('crates/kernel/src/checking.rs',
                             'if advance(&mut w, i, &mut out) { return false; }',
                             'if !advance(&mut w, i, &mut out) { return false; }',
                             '//crates/kernel:verus_checking_is_settled'),
    'phase_witness_resume': ('crates/kernel/src/witness.rs',
                             'command: Command::Resume,',
                             'command: Command::Pause,',
                             '//crates/kernel:verus_witness'),
    'phase_late_resume': ('crates/kernel/src/witness.rs',
                           'if n.paused && !n.phase.is_stopping() {',
                           'if n.paused && !n.phase.is_stopping() && n.phase != Phase::Publishing {',
                           '//crates/kernel:verus_witness'),
    'phase_stopping_ack': ('crates/kernel/src/witness.rs',
                            'if let Some(e) = n.effect {',
                            'if let Some(e) = if n.phase.is_stopping() { None } else { n.effect } {',
                            '//crates/kernel:verus_witness'),
    'phase_closed_sibling': ('crates/kernel/src/witness.rs',
                              'if n.phase.is_closed() {\n                i += 1;\n                continue;\n            }',
                              'if n.phase.is_closed() { return None; }',
                              '//crates/kernel:verus_witness'),
    'unplanned_selector_order': ('crates/kernel/src/witness.rs',
                                 'if n.paused && !n.phase.is_stopping() {',
                                 'if n.paused && !n.phase.is_stopping() && n.effect.is_none() {',
                                 '//crates/kernel:verus_witness'),
    'unplanned_admission': ('crates/kernel/src/ranks.rs',
                            'set_phase(m, Phase::Planning { must_split: false });',
                            'set_phase(m, Phase::Planning { must_split: true });',
                            '//crates/kernel:verus_ranks_advance_admit'),
    'unplanned_waiting': ('crates/kernel/src/scheduler.rs',
                          'context.admit = w.dependencies_delivered(NodeId(i as u64)) && admits(w, n.repo);',
                          'context.admit = admits(w, n.repo);',
                          '//crates/kernel:verus_scheduler'),
    'unplanned_cover_occupant': ('crates/kernel/src/scheduler.rs',
                                 'context.slot = slot_free(w, i) && first_in_line(w, i);',
                                 'context.slot = false;',
                                 '//crates/kernel:verus_scheduler'),

    'allocation_stopping_outcome': ('crates/kernel/src/allocation.rs',
                                     'm.phase = Phase::Stopping(Outcome::Merged);',
                                     'm.phase = Phase::Stopping(Outcome::Stopped);',
                                     '//crates/kernel:verus_allocation_enter_stopping'),
    'allocation_incident_rob': ('crates/kernel/src/allocation.rs',
                               'm.incident_needs_rob = needs_rob;',
                               'm.incident_needs_rob = !needs_rob;',
                               '//crates/kernel:verus_allocation_raise_incident'),
    'facts_transition_closed_priority': ('crates/kernel/src/facts.rs',
                                         'if !w.nodes[i].phase.is_closed() {',
                                         'if true {',
                                         '//crates/kernel:verus_facts_transition'),
    'facts_ci_failed_dispatch': ('crates/kernel/src/facts.rs',
                                 'ci_infrastructure(w, i, out);',
                                 'failed_round(w, i, ReviewStop::CiFailed, out);',
                                 '//crates/kernel:verus_facts_ci_failed'),
    'kernel_cont_rank_stop': ('crates/kernel/src/lib.rs',
                              'Cont::Stop(_) | Cont::Nowhere => 2,',
                              'Cont::Stop(_) | Cont::Nowhere => 3,',
                              '//crates/kernel:verus_kernel_cont_rank'),
    'events_ack_skipped': ('crates/kernel/src/events.rs',
                           'on_ack(w, i, seq, ack, out);',
                           'let _ = (i, seq, ack);',
                           '//crates/kernel:verus_events'),
    'events_adoption_storage': ('crates/kernel/src/events.rs',
                                'node.proposed = Some(head);',
                                'node.proposed = Some(head);\n                w.file_sets = Vec::new();',
                                '//crates/kernel:verus_events'),
    'cleanup_mark_handled_pos': ('crates/kernel/src/command_helpers.rs',
                                 'm.pos += 1;', 'm.pos += 0;',
                                 '//crates/kernel:verus_command_helpers_mark_handled'),
    'cleanup_confirmed_agenda_skipped': ('crates/kernel/src/acknowledgements.rs',
                                        'confirmed_agenda(m, kind);', 'let _ = kind;',
                                        '//crates/kernel:verus_acknowledgements_confirmed'),
    'reply_ingest_budget': ('crates/kernel/src/ingest.rs',
                            'm.credit.start = size as u64 + RESERVE;',
                            'm.credit.start = size as u64 + RESERVE + 259;',
                            '//crates/kernel:verus_ingest_finish_ingest_node'),
    'timing3_ingest_deposit': ('crates/kernel/src/ingest.rs',
                               'm.credit.start = size as u64 + RESERVE;',
                               'm.credit.start = size as u64;',
                               '//crates/kernel:verus_ingest_finish_ingest_node'),
    'timing3_pause_release': ('crates/kernel/src/lib.rs',
                             'm.holds = false;\n                set_phase(m, Phase::Ready(r));',
                             'm.holds = true;\n                set_phase(m, Phase::Ready(r));',
                             '//crates/kernel:verus_kernel_pause_node'),

    'facts_merge_unapproved': ('crates/kernel/src/facts.rs',
                               '    if !approved {',
                               '    if approved {',
                               '//crates/kernel:verus_facts_merge'),
    'review_admitted_release': ('crates/kernel/src/reply_admission.rs',
                                '    end_operation(&mut w.nodes[i]);',
                                '    let _ = i;',
                                '//crates/kernel:verus_review_admission_admitted_reply'),
    'review_admitted_duplicate': ('crates/kernel/src/reply_admission.rs',
                                  '            reply_transition(w, i, reply, out);',
                                  '            reply_transition(w, i, reply, out);\n'
                                  '            reply_transition(w, i, reply, out);',
                                  '//crates/kernel:lib_test_tl3_kernel'),
    'advance_ticket_proposed': ('crates/kernel/src/ranks.rs',
                                'take_ticket_node(node, next_ticket);\n            return true;\n        }\n        if context.pool',
                                'take_ticket_node(node, next_ticket);\n            node.proposed = Some(HeadId(7));\n            return true;\n        }\n        if context.pool',
                                '//crates/kernel:verus_ranks_advance_start'),
    'advance_admission_withheld': ('crates/kernel/src/ranks.rs',
                                   'm.credit.withheld = false;',
                                   'm.credit.withheld = true;',
                                   '//crates/kernel:verus_ranks_advance_admit'),
    'advance_closed_review_stop': ('crates/kernel/src/ranks.rs',
                                   'if n.phase.is_closed() {\n            return false;\n        }',
                                   'if n.phase.is_closed() {\n            node.review_stop = Some(ReviewStop::PlanCantMeet);\n            return false;\n        }',
                                   '//crates/kernel:verus_ranks_advance_blocked'),
    'advance_slot_adoption': ('crates/kernel/src/ranks.rs',
                              'm.holds = true;\n            set_phase(m, r.phase());',
                              'm.holds = true;\n            m.adopted = true;\n            set_phase(m, r.phase());',
                              '//crates/kernel:verus_ranks_advance_slot'),
    'advance_checks_output_node': ('crates/kernel/src/ranks.rs',
                                   'emit_node(node, context.node, EffectKind::SubmitChecks, out);',
                                   'emit_node(node, context.node ^ 1, EffectKind::SubmitChecks, out);',
                                   '//crates/kernel:verus_ranks_advance_checks'),
    'advance_gate_capacity': ('crates/kernel/src/scheduler.rs',
                               'holders < gate.w as u32', 'holders <= gate.w as u32',
                               '//crates/kernel:verus_scheduler_slot_free'),
    'advance_gate_overlap': ('crates/kernel/src/scheduler.rs',
                              'theirs < ticket && !w.footprints_disjoint(older.footprint, waiter.footprint)',
                              'theirs <= ticket && !w.footprints_disjoint(older.footprint, waiter.footprint)',
                              '//crates/kernel:verus_scheduler_older_overlap'),
    'advance_gate_line': ('crates/kernel/src/scheduler.rs',
                           'if theirs >= first_runnable || !slot_free(w, index)',
                           'if theirs <= first_runnable || !slot_free(w, index)',
                           '//crates/kernel:verus_scheduler_first_in_line'),

    'review_approval_head': ('crates/kernel/src/work_replies.rs',
        '            m.approved = Some(head);',
        '            m.approved = None;',
        '//crates/kernel:verus_work_replies'),
    'review_reviewer_restart': ('crates/kernel/src/work_replies.rs',
        '                w.nodes[i].review_restarted = true;',
        '                w.nodes[i].review_restarted = false;',
        '//crates/kernel:verus_work_replies'),
    'review_free_fix_round': ('crates/kernel/src/retries.rs',
        '        m.round = 1;',
        '        m.round = 0;',
        '//crates/kernel:verus_review_retry_round'),
    'review_recheck_charge': ('crates/kernel/src/retries.rs',
        '    let free = m.rechecks <= FREE_RECHECKS;',
        '    let free = false;',
        '//crates/kernel:verus_review_retry_recheck'),
    'review_rebase_head': ('crates/kernel/src/work_replies.rs',
        '            update_recheck_head(&mut w.nodes[i], head);',
        '            update_recheck_head(&mut w.nodes[i], HeadId(0));',
        '//crates/kernel:verus_work_replies_rebase'),
    'review_conflict_credit': ('crates/kernel/src/work_replies.rs',
        '                m.credit.spend(1);',
        '                let _ = 1;',
        '//crates/kernel:verus_work_replies_rebase'),
    'review_failed_generation': ('crates/kernel/src/retries.rs',
        '    m.generation += 1;',
        '    m.generation += 0;',
        '//crates/kernel:verus_retries'),
    'review_fast_stop_reason': ('crates/kernel/src/retries.rs',
        '    m.review_stop = Some(step);',
        '    m.review_stop = None;',
        '//crates/kernel:verus_review_retry_stop'),
    'review_checks_head': ('crates/kernel/src/work_replies.rs',
        '                m.checked = m.head;',
        '                m.checked = None;',
        '//crates/kernel:verus_work_replies'),
    'review_fast_round_output': ('crates/kernel/src/retries.rs',
        '        next_rung(w, i, step, out);',
        '        next_rung(w, i, step, out);\n'
        '        notice(out, i, Notice::Note(NoteKind::Replanned));',
        '//crates/kernel:verus_review_retry_round'),

    'review_room_credit': ('crates/kernel/src/retries.rs',
        '        m.plan_retries = 0;\n        set_phase(m, Phase::Planning { must_split: true });',
        '        m.plan_retries = 1;\n        set_phase(m, Phase::Planning { must_split: true });',
        '//crates/kernel:verus_review_retry_room'),
    'review_recheck_count': ('crates/kernel/src/retries.rs',
        '    m.rechecks = m.rechecks.saturating_add(1);',
        '    m.rechecks = 0;',
        '//crates/kernel:verus_review_retry_recheck'),

}


def run(workspace, target, name):
    statuses = workspace.test([target], 'remote', {'ROLE': 'KERNEL_SAFETY_QUALIFICATION'},
                              name, extra=['--jobs=16', '--nocache_test_results', '--noremote_accept_cached'])
    log = workspace.root / ('bazel-testlogs/' + target[2:].replace(':', '/') + '/test.log')
    text = log.read_text() if log.is_file() else ''
    print(text, flush=True)
    return statuses, text


def libtest_summaries(log):
    summaries = []
    for line in log.splitlines():
        marker = line.find('] {')
        if marker < 0:
            continue
        try:
            event = json.loads(line[marker + 2:])
        except ValueError:
            continue
        if event.get('type') == 'suite':
            summaries.append(event)
    return summaries


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('case', choices=CASES)
    parser.add_argument('--expected-head', help='require this exact baseline before seeding')
    args = parser.parse_args()
    workspace = bazel_gate.Workspace()
    base = workspace.run('git', 'rev-parse', 'HEAD').strip()
    filename, before, after, target = CASES[args.case]
    if args.expected_head:
        assert base == args.expected_head, 'qualification baseline differs from the requested final head'
    report = {'case': args.case, 'head': base, 'target': target}
    try:
        statuses, log = run(workspace, target, 'safety-baseline-' + args.case)
        assert statuses.get(target) == 'PASSED', statuses
        proof = ':verus_' in target
        if proof:
            assert 'VERUS_PROOF' in log
        else:
            assert any(event.get('event') == 'ok' and event.get('passed', 0) > 0
                       for event in libtest_summaries(log)), (
                'controller qualification must execute nonzero tests')
        path = workspace.root / filename
        source = path.read_text()
        assert source.count(before) == 1, 'mutant must select exactly one source expression'
        path.write_text(source.replace(before, after, 1))
        workspace.run('git', '-c', 'user.name=Kernel safety qualification', '-c',
                      'user.email=kernel-safety@invalid', 'commit', '--quiet', '--no-verify',
                      '-am', 'Seed ' + args.case + ' in the executable source\n\n'
                      'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>')
        report['mutant'] = workspace.run('git', 'rev-parse', 'HEAD').strip()
        report['mutant_parent'] = workspace.run('git', 'rev-parse', 'HEAD^').strip()
        assert report['mutant_parent'] == base, 'mutant must be a direct child of the baseline'
        statuses, log = run(workspace, target, 'safety-mutant-' + args.case)
        assert statuses.get(target) == 'FAILED', statuses
        if proof:
            assert any(message in log for message in (
                'postcondition not satisfied', 'precondition not satisfied',
                'arithmetic overflow', 'invariant not satisfied', 'assertion failed',
                'Call to non-static function fails to satisfy `callee.requires(args)`',
            )), 'mutant must fail a real proof obligation'
            assert re.search(r'verification results:: [0-9]+ verified, [1-9][0-9]* errors', log)
        else:
            assert any(event.get('event') == 'failed' and event.get('failed', 0) > 0
                       for event in libtest_summaries(log)) and 'panicked at' in log, (
                'boundary mutant must fail an executed assertion, not compilation')
        verdicts, failed = bazel_gate.classify([target], statuses, {target})
        assert verdicts['rust'] == 'fail' and failed['rust'] == [target]
        outputs = remote_gate.outputs({'mode': 'pr', 'verdicts': verdicts})
        assert outputs['rust'] == 'failure'
        try:
            remote_gate.aggregate('rust', {'bazel': {'result': 'failure',
                'outputs': outputs | {'scopes_uploaded': 'true'}}})
        except RuntimeError:
            pass
        else:
            raise AssertionError('required Rust aggregate accepted the mutant')
        report['rust_verdict'] = verdicts['rust']
        report['failed_rust_targets'] = failed['rust']
        report['required_rust_check'] = 'failure'
        report['passed'] = True
    finally:
        workspace.checkout(base)
        output = Path(os.environ['BUILDBUDDY_ARTIFACTS_DIRECTORY'])
        output.mkdir(parents=True, exist_ok=True)
        (output / ('safety-' + args.case + '.json')).write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report), flush=True)


if __name__ == '__main__':
    main()
