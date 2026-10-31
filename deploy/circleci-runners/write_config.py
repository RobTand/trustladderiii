# Runs on the target host. Raw API output travels only through stdin.
import json, os, pathlib, socket, sys
response = json.load(sys.stdin)
if 'data' not in response:
    raise SystemExit('Runner token creation failed; response withheld')
token = response['data']['attributes']['token']
slot = int(sys.argv[1])
directory = pathlib.Path.home()/'.config/circleci-fleet'
directory.mkdir(mode=0o700, exist_ok=True)
path = directory/f'runner-{slot}.yaml'
config = f'''api:
  auth_token: {json.dumps(token)}
runner:
  name: {socket.gethostname()}-circleci-{slot}
  mode: single-task
  working_directory: /work/jobs
  cleanup_working_directory: true
  task_agent_directory: /opt/circleci
  command_prefix: ["/usr/local/bin/circleci-task"]
  max_run_time: 50m
logging:
  level: info
  format: text
'''
fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o600)
with os.fdopen(fd,'w') as f: f.write(config)
os.chmod(path,0o600)
print(f'Stored runner config slot {slot}, mode {path.stat().st_mode & 0o777:o}; token withheld')
