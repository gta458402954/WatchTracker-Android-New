import { execFileSync, spawnSync } from 'node:child_process';

function git(...args) {
  return execFileSync('git', args, { encoding: 'utf8' }).trim();
}

const status = git('status', '--porcelain');
if (status) {
  console.error('便携版打包已取消：Git 工作区存在未提交修改。请先检查并提交本地 Git。');
  console.error(status);
  process.exit(1);
}

const commit = git('rev-parse', 'HEAD');
const commitTime = git('show', '-s', '--format=%cI', 'HEAD');
console.log(`正在从 Git 提交 ${commit.slice(0, 8)}（提交时间 ${commitTime}）构建便携版。`);

const npmCli = process.env.npm_execpath;
if (!npmCli) {
  console.error('无法定位当前 npm CLI，便携版打包已取消。');
  process.exit(1);
}

const result = spawnSync(process.execPath, [npmCli, 'run', 'tauri', 'build'], {
  stdio: 'inherit',
  env: {
    ...process.env,
    WATCHTRACKER_GIT_COMMIT: commit,
    WATCHTRACKER_GIT_COMMIT_TIME: commitTime,
  },
});

if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}

process.exit(result.status ?? 1);
