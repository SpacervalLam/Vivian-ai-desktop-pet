import assert from 'node:assert/strict';

/**
 * 时序契约模拟（不是真实运行时）：把 `dock.rs` 的裁决语义在 JS 里复刻一份，
 * 锁死「时灵时不灵」那几个乱序场景。真实运行时的验收仍看 app 日志
 * `[LLM-IO] >>> task=... tools=`、窗口 region 的实际状态与实机拖拽。
 *
 * 契约三条：
 * 1. 是否贴边只由窗口几何裁决，不等 60ms 采样、不等前端事件；
 * 2. 每次状态推进都带 epoch，迟到的动画帧一律不再写窗口 / region；
 * 3. 恢复完整窗口是后端的一次同步动作，前端恢复路径零窗口写入。
 */

function controller() {
  let epoch = 0;
  let mode = 'Off';
  let dockedOverflow = 0;
  let entering = false;
  let undocks = 0;
  let regionInstalled = false;
  let windowWrites = 0;

  return {
    snapshot: () => ({ epoch, mode, undocks, regionInstalled, windowWrites }),
    markDocked(overflow) {
      epoch += 1; mode = 'Docked'; dockedOverflow = overflow; entering = true; regionInstalled = true;
      return epoch;
    },
    beginDocking() { epoch += 1; mode = 'Docking'; return epoch; },
    promoteDocked(overflow) {
      if (mode === 'Docking') { mode = 'Docked'; dockedOverflow = overflow; entering = true; }
      return epoch;
    },
    settle(overflow) {
      if (mode !== 'Docked' || !entering) return 'Ignored';
      if (overflow !== null && overflow > 0) { entering = false; dockedOverflow = overflow; return 'Docked'; }
      // 落位时窗口已经不在边缘：调用方（settle_pet_dock）负责 leave_edge。
      this.leaveEdge();
      return 'LeftEdge';
    },
    leaveEdge() {
      if (mode === 'Off') return null;
      epoch += 1; mode = 'Off'; entering = false; undocks += 1; regionInstalled = false;
      return epoch;
    },
    observe(overflow) {
      if (mode !== 'Docked') return 'None';
      // 入场动画还没落位：窗口会被收回到贴边位姿（越界量可能变小），不能裁决。
      if (entering) return 'None';
      if (overflow === null || overflow < dockedOverflow - 0.5) {
        this.leaveEdge();
        return 'LeftEdge';
      }
      return 'None';
    },
    accept(candidate) {
      return candidate > 0 ? candidate === epoch : mode !== 'Off';
    },
    writeFrame(candidate, atEdge) {
      if (!this.accept(candidate)) return false;
      windowWrites += 1;
      if (atEdge) regionInstalled = true;
      return true;
    },
  };
}

// 场景 1：拖离边缘后，旧动画帧与 dock:changed{Off} 任意顺序到达，终态一致。
for (const order of [['frame', 'undock'], ['undock', 'frame']]) {
  const c = controller();
  const epoch = c.markDocked(60);
  assert.equal(c.settle(60), 'Docked', '入场落位');
  assert.equal(c.observe(59), 'LeftEdge', 'SetWindowPos 同步裁决');
  let staleAccepted = false;
  for (const step of order) {
    if (step === 'frame') staleAccepted = c.writeFrame(epoch, true);
    else assert.equal(c.snapshot().mode, 'Off');
  }
  assert.equal(staleAccepted, false, '迟到帧不得重装 region');
  assert.equal(c.snapshot().regionInstalled, false);
  assert.equal(c.snapshot().undocks, 1);
}

// 场景 2：mouseup 与 60ms tick 的先后不影响恢复结果。
for (const mouseupFirst of [true, false]) {
  const c = controller();
  c.markDocked(60);
  c.settle(60);
  const events = mouseupFirst ? ['mouseup', 'tick'] : ['tick', 'mouseup'];
  for (const step of events) {
    if (step === 'tick') c.observe(59);
  }
  assert.equal(c.snapshot().mode, 'Off');
  assert.equal(c.snapshot().undocks, 1);
  assert.equal(c.snapshot().regionInstalled, false);
  assert.equal(c.snapshot().windowWrites, 0, '恢复路径没有写窗口');
}

// 场景 3：入场达标不推进 epoch，后续动画帧继续被接受（窗口不会停在半路）。
{
  const c = controller();
  const epoch = c.beginDocking();
  assert.equal(c.promoteDocked(60), epoch);
  assert(c.writeFrame(epoch, true));
  assert(c.writeFrame(epoch, true));
  assert.equal(c.snapshot().mode, 'Docked');
  assert.equal(c.snapshot().regionInstalled, true);
}

// 场景 4：用户中途抓起取消入场，旧走路帧一律失效。
{
  const c = controller();
  const epoch = c.beginDocking();
  assert(c.writeFrame(epoch, false));
  c.leaveEdge();
  assert.equal(c.writeFrame(epoch, true), false);
  assert.equal(c.snapshot().mode, 'Off');
  assert.equal(c.snapshot().regionInstalled, false);
}

// 场景 5：沿边缘平移、往屏外再推一点都不算离开；往屏内 1px 才算。
{
  const c = controller();
  c.markDocked(60);
  c.settle(60);
  assert.equal(c.observe(60), 'None');
  assert.equal(c.observe(61), 'None');
  assert.equal(c.observe(59), 'LeftEdge');
}

// 场景 6：把桌宠「按入」边缘越深越容易回弹的根因。
// 松手时已推出 200px（比贴边位姿 106px 更深），入场动画必须把窗口收回来，
// 越界量 200 → 190 → … → 106 一路变小。入场期间不得判离开，落位后阈值定在 106。
{
  const c = controller();
  c.markDocked(200);
  for (const overflow of [190, 150, 120, 106]) {
    assert.equal(c.observe(overflow), 'None', `入场回收 ${overflow}px 不能被当成离开边缘`);
  }
  assert.equal(c.settle(106), 'Docked');
  assert.equal(c.observe(106), 'None', '落在贴边位姿上仍算贴边');
  assert.equal(c.observe(105), 'LeftEdge', '落位后往屏内 1px 才算离开');
  assert.equal(c.snapshot().undocks, 1);
}

// 场景 7：入场动画被打断、窗口已被挪回屏幕内 → 落位结算直接回到完整窗口。
{
  const c = controller();
  c.markDocked(200);
  assert.equal(c.settle(null), 'LeftEdge');
  assert.equal(c.snapshot().mode, 'Off');
  assert.equal(c.snapshot().regionInstalled, false);
}

console.log('Dock event-order invariants passed (geometry-first, epoch, zero-write restore, entry grace).');
