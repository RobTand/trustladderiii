"""#771 qualification driver. Run only inside an admitted PrismaBuild action."""

import argparse
import hashlib
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


CASES = [
    ('command_id', 'lib.rs', 'self.last_cmd.checked_add(1)', 'self.last_cmd.checked_add(0)'),
    ('credit_balance', 'lib.rs', 'if self.withheld {\n                0', 'if false {\n                0'),
    ('credit_ledger', 'lib.rs',
     'self.start as i64 + self.earned as i64 + self.added as i64 - self.spent as i64',
     'self.start as i64 + self.earned as i64 - self.spent as i64'),
    ('credit_spend', 'lib.rs', 'self.spent += cost;', 'self.spent += cost + 1;'),
    ('credit_topup', 'lib.rs', 'self.added += short as u32;', 'self.added += 0;'),
    ('agenda', 'lib.rs', 'Phase::Unplanned => &[I::Admit],', 'Phase::Unplanned => &[],'),
    ('phase_rank', 'lib.rs', 'Phase::Unplanned => 28,', 'Phase::Unplanned => 27,'),
    ('distance', 'ranks.rs', 'd += 8;', 'd += 7;'),
    ('pending_rank', 'ranks.rs', 'return 3;', 'return 2;'),
    ('sum_update', 'ranks.rs',
     'node_total(nodes.drop_last()) + distance(nodes.last())',
     'node_total(nodes.drop_last()) + 0'),
    ('credit_deposit', 'lib.rs', 'self.added += deposit as u32;', 'self.added += 0;'),
    ('phase_generation', 'lib.rs', 'm.generation += 1;', 'm.generation += 0;', 'set_phase'),
    ('merge_credit', 'lib.rs', 'm.credit.earned += 1;', 'm.credit.earned += 0;', 'merge_credit'),
    ('effect_sequence', 'lib.rs', 'm.next_seq += 1;', 'm.next_seq += 0;', 'emit_node'),
    ('ticket_allocation', 'lib.rs', '*next_ticket += 1;', '*next_ticket += 0;', 'take_ticket_node'),
    ('outbox_length', 'lib.rs', 'self.len += 1;', 'self.len += 0;', 'push'),
    ('scan_budget', 'headroom.rs', 'checked_add(1)', 'checked_add(0)', 'scan_step_budget'),
    ('settle_budget', 'headroom.rs', 'checked_mul(6)', 'checked_mul(5)', 'settle_step_budget'),
    ('ticket_budget', 'headroom.rs', 'checked_mul(2)', 'checked_mul(1)', 'ticket_step_budget'),
    ('local_capacities', 'ranks.rs',
     'Some(Item::Teardown(_)) => if n.question.is_some() { 0 } else { (6 - n.pos) as nat }',
     'Some(Item::Teardown(_)) => if n.question.is_some() { 0 } else { (7 - n.pos) as nat }'),
    ('settle_blocked', 'ranks.rs', 'node.ticket = None;', 'let _ = node.ticket;', 'advance_blocked'),
    ('settle_finished', 'ranks.rs', 'set_phase(node, Phase::Queued);',
     'let _ = Phase::Queued;', 'advance_finished'),
    ('settle_do', 'ranks.rs', 'emit_node(node, context.node, kind, out);',
     'let _ = kind;', 'advance_do'),
    ('settle_start', 'ranks.rs', 'take_ticket_node(node, next_ticket);',
     'let _ = next_ticket;', 'advance_start'),
    ('settle_checks', 'ranks.rs', 'm.pos += 1;', 'm.pos += 0;', 'advance_checks'),
    ('settle_rebase', 'ranks.rs', 'm.pos += 1;', 'm.pos += 0;', 'advance_rebase'),
    ('settle_admit', 'ranks.rs', 'set_phase(m, Phase::Planning { must_split: false });',
     'let _ = m;', 'advance_admit'),
    ('settle_slot', 'ranks.rs', 'take_ticket_node(node, next_ticket);',
     'let _ = next_ticket;', 'advance_slot'),
    ('settle_release', 'ranks.rs', 'm.pos += 1;', 'm.pos += 0;', 'advance_release'),
    ('settle_teardown', 'ranks.rs', 'm.pos += 1;', 'm.pos += 0;', 'advance_teardown'),
    ('settle_idle', 'ranks.rs', 'false', 'true', 'advance_idle'),
    ('settle_dispatch', 'ranks.rs',
     'Item::Admit => advance_admit(node, next_ticket, context, out)',
     'Item::Admit => advance_idle(node, next_ticket, context, out)', 'advance_node'),
    ('headroom_generation', 'headroom.rs',
     'node.generation > u64::MAX - GENERATION_STEP_BUDGET',
     'node.generation > u64::MAX - (GENERATION_STEP_BUDGET - 1)', 'node_counter_exhaustion'),
    ('headroom_question', 'headroom.rs',
     'self.next_question > u64::MAX - QUESTION_STEP_BUDGET',
     'self.next_question >= u64::MAX - QUESTION_STEP_BUDGET', 'counter_exhaustion'),
    ('headroom_open_count', 'headroom.rs',
     'if !self.nodes[index].phase.is_closed()',
     'if self.nodes[index].phase.is_closed()', 'counter_exhaustion'),
    ('potential_sum', 'ranks.rs',
     '+ potential(nodes[end as int - 1])', '+ 0'),
    ('pool_count', 'lib.rs', 'runs += 1;', 'runs += 0;', 'pool_free'),
    ('admission_count', 'lib.rs', 'waiting += 1;', 'waiting += 0;', 'admits'),
    ('question_allocation', 'lib.rs', 'w.next_question += 1;',
     'w.next_question += 2;', 'ask_rob'),
    ('incident_generation', 'lib.rs', 'set_phase(m, Phase::Ready(r));',
     'let _ = r;', 'raise_incident'),
    ('run_retry_bound', 'lib.rs', 'm.generation += 1;', 'm.generation += 3;', 'run_failed'),
    ('delivery_retry_bound', 'lib.rs', 'm.generation += 1;', 'm.generation += 3;', 'failed_delivery_node'),
    ('prerequisite_query', 'dependencies.rs',
     'd.rejected || d.status != DependencyStatus::Delivered',
     'd.rejected && d.status != DependencyStatus::Delivered', 'dependencies_delivered'),
    ('reply_ingest_budget', 'lib.rs', 'm.credit.start = size as u32 + RESERVE;',
     'm.credit.start = size as u32 + RESERVE + 259;', 'finish_ingest_node'),
    ('reply_plan_budget', 'lib.rs', 'set_phase(m, Phase::Ready(Resume::Making));',
     'set_phase(m, Phase::Ready(Resume::Making)); m.generation += 1;', 'reply_plan'),
    ('reply_work_budget', 'lib.rs', 'm.credit.spend(1);',
     'm.credit.spend(2);', 'reply_work'),
    ('fact_budget', 'lib.rs', 'set_phase(m, Phase::Ready(Resume::Publishing));',
     'set_phase(m, Phase::Ready(Resume::Publishing)); m.generation += 1;', 'fact_transition'),
    ('late_merge_budget', 'lib.rs', 'merge_credit(m);',
     'merge_credit(m); m.credit.earned += 1;', 'pr_merged'),
    ('file_collection', 'footprint.rs', 'files.push(FileId(bit));',
     'files.push(FileId(0));', 'collect_files'),
    ('file_table_frame', 'footprint.rs', 'self.file_sets.push(files);',
     'self.file_sets.push(files); self.next_ticket += 1;', 'intern_sorted_files'),
    ('file_union_frame', 'footprint.rs', 'self.intern_sorted_files(files)',
     '{ self.next_question += 1; self.intern_sorted_files(files) }', 'union_footprints'),
    ('dependency_visit_fuel', 'dependencies.rs',
     '+ if seen[end as int - 1] { 0nat } else { 1nat }', '+ 0nat'),
    ('dependency_traversal_fuel', 'dependencies.rs', 'seen |= mask;',
     'seen ^= mask;', 'dependency_path'),
    ('dependency_removal_frame', 'dependencies.rs', 'self.dependencies.truncate(write);',
     'self.dependencies.truncate(write); self.next_ticket += 1;', 'remove_dependency'),
    ('dependency_fact_frame', 'dependencies.rs', 'self.withhold_dependency_credit(i);',
     'self.withhold_dependency_credit(i); self.next_question += 1;', 'record_dependency'),
]


def verify(binary, sysroot, root, selected=None):
    command = [str(binary), '--num-threads', '1', '--rlimit', '30', '-V', 'spinoff-all',
               '--sysroot', str(sysroot), str(root), '--edition', '2024']
    if selected:
        module, function = selected
        owner = {
            'push': 'Outbox',
            'ticket_step_budget': 'World',
            'counter_exhaustion': 'World',
            'dependencies_delivered': 'World',
            'collect_files': 'World',
            'intern_sorted_files': 'World',
            'union_footprints': 'World',
            'dependency_path': 'World',
            'remove_dependency': 'World',
            'dependency_fact': 'World',
            'record_dependency': 'World',
        }.get(function)
        target = f'{owner}::{function}' if owner else function
        command.extend(['--verify-only-module', module, '--verify-function', target])
    return subprocess.run(
        command,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, check=False,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('verifier', type=Path)
    parser.add_argument('sysroot', type=Path)
    parser.add_argument('--case', choices=[case[0] for case in CASES], action='append')
    parser.add_argument('--full-mutants', action='store_true',
                        help='reverify every function for each mutant, after the full baseline')
    args = parser.parse_args()
    kernel = Path(__file__).resolve().parents[1]
    baseline = verify(args.verifier, args.sysroot, kernel / 'verus/progress.rs')
    print(baseline.stdout, flush=True)
    if baseline.returncode != 0 or not re.search(
        r'verification results:: [1-9][0-9]* verified, 0 errors', baseline.stdout
    ):
        raise SystemExit('baseline did not verify; mutant rejection would be ambiguous')
    for name, filename, old, new, *scope in CASES:
        if scope:
            owners = [path for path in (kernel / 'src').glob('*.rs')
                      if f'fn {scope[0]}(' in path.read_text()]
            if len(owners) != 1:
                raise SystemExit(f'{name}: function must have exactly one executable owner')
            filename = owners[0].name
        if args.case and name not in args.case:
            continue
        with tempfile.TemporaryDirectory(prefix=f't771-{name}-') as scratch:
            copy = Path(scratch) / 'crates/kernel'
            shutil.copytree(kernel, copy)
            boundary = copy.parent / 'controller/src'
            boundary.mkdir(parents=True)
            shutil.copy2(kernel.parent / 'controller/src/kernel_boundary.rs', boundary)
            source = copy / 'src' / filename
            before = source.read_text()
            start = before.index(f'fn {scope[0]}(') if scope else 0
            end = before.find('kernel_fn! {', start) if scope else -1
            if end < 0:
                end = len(before)
            selected = before[start:end]
            if old not in selected:
                raise SystemExit(f'{name}: seed no longer matches the selected source')
            mutated = before[:start] + selected.replace(old, new, 1) + before[end:]
            source.write_text(mutated)
            digest = hashlib.sha256(mutated.encode()).hexdigest()
            # The unmodified full baseline above proves every callee contract.
            # A scoped mutant changes only this function's actual body. Recheck
            # that body against its contract; unscoped seeds still recheck all.
            selected = None
            if scope and not args.full_mutants:
                module = 'kernel' if filename in {'lib.rs', 'headroom.rs'} else (
                    'kernel::' + Path(filename).stem)
                selected = (module, scope[0])
            result = verify(args.verifier, args.sysroot, copy / 'verus/progress.rs', selected)
            print(f'CASE {name} source_sha256={digest} target={selected} '
                  f'exit={result.returncode}', flush=True)
            print(result.stdout, flush=True)
            rejected = result.returncode != 0 and re.search(
                r'verification results:: [0-9]+ verified, [1-9][0-9]* errors', result.stdout
            ) and re.search(
                r'precondition not satisfied|postcondition not satisfied|assertion failed|invariant not satisfied|arithmetic overflow|arithmetic underflow',
                result.stdout,
            )
            if not rejected:
                raise SystemExit(f'{name}: expected a failed proof obligation, not a frontend error')
            print(f'REJECTED {name}', flush=True)


if __name__ == '__main__':
    main()
