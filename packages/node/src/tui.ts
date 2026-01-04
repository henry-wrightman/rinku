import * as readline from 'readline';
import { cpus } from 'os';
import { TelemetryService, SystemTelemetry, formatBytes, formatDuration } from './telemetry.js';
import type { Consensus } from './consensus.js';
import type { StateManager } from './state.js';
import type { GossipService } from './gossip.js';
import type { GasService } from './gas.js';
import type { PeerSyncService } from './peerSync.js';

const COLORS = {
  reset: '\x1b[0m',
  bold: '\x1b[1m',
  dim: '\x1b[2m',
  cyan: '\x1b[36m',
  green: '\x1b[32m',
  yellow: '\x1b[33m',
  magenta: '\x1b[35m',
  blue: '\x1b[34m',
  red: '\x1b[31m',
  white: '\x1b[37m',
  bgBlue: '\x1b[44m',
  bgBlack: '\x1b[40m'
};

type TuiView = 'dashboard' | 'logs' | 'peers' | 'dag' | 'specs' | 'threads';

interface TuiDeps {
  consensus: Consensus;
  state: StateManager;
  gossip: GossipService | null;
  peerSync: PeerSyncService | null;
  gas: GasService | null;
  telemetry: TelemetryService;
  getCheckpointHeight: () => number;
  getPendingCount: () => number;
  getWitnessedCount: () => number;
  getCryptoWorkers: () => number;
  setCryptoWorkers: (count: number) => void;
  nodeId: string;
  version: string;
  protocolVersion?: string;
}

export class NodeTui {
  private deps: TuiDeps;
  private currentView: TuiView = 'dashboard';
  private refreshInterval: NodeJS.Timeout | null = null;
  private logBuffer: string[] = [];
  private maxLogLines = 100;
  private isActive = false;
  private originalConsoleLog: typeof console.log;
  private originalConsoleError: typeof console.error;
  private originalConsoleWarn: typeof console.warn;
  private tpsHistory: number[] = [];
  private lastTxCount = 0;
  private lastTpsCheck = Date.now();

  constructor(deps: TuiDeps) {
    this.deps = deps;
    this.originalConsoleLog = console.log.bind(console);
    this.originalConsoleError = console.error.bind(console);
    this.originalConsoleWarn = console.warn.bind(console);
  }

  start(): void {
    if (this.isActive) return;
    this.isActive = true;

    this.interceptConsole();

    readline.emitKeypressEvents(process.stdin);
    if (process.stdin.isTTY) {
      process.stdin.setRawMode(true);
    }

    process.stdin.on('keypress', this.handleKeypress.bind(this));

    this.refreshInterval = setInterval(() => {
      if (this.currentView === 'dashboard') {
        this.renderDashboard();
      } else if (this.currentView === 'peers') {
        this.renderPeers();
      } else if (this.currentView === 'specs') {
        this.renderSpecs();
      } else if (this.currentView === 'threads') {
        this.renderThreads();
      }
    }, 1000);

    this.renderDashboard();
  }

  stop(): void {
    if (!this.isActive) return;
    this.isActive = false;

    if (this.refreshInterval) {
      clearInterval(this.refreshInterval);
      this.refreshInterval = null;
    }

    this.restoreConsole();

    if (process.stdin.isTTY) {
      process.stdin.setRawMode(false);
    }

    this.clearScreen();
  }

  private interceptConsole(): void {
    const addToBuffer = (prefix: string, ...args: unknown[]) => {
      const timestamp = new Date().toISOString().slice(11, 19);
      const message = args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' ');
      this.logBuffer.push(`${COLORS.dim}${timestamp}${COLORS.reset} ${prefix}${message}`);
      if (this.logBuffer.length > this.maxLogLines) {
        this.logBuffer.shift();
      }
    };

    console.log = (...args: unknown[]) => {
      addToBuffer('', ...args);
      if (this.currentView === 'logs') {
        this.renderLogs();
      }
    };

    console.error = (...args: unknown[]) => {
      addToBuffer(`${COLORS.red}[ERR]${COLORS.reset} `, ...args);
      if (this.currentView === 'logs') {
        this.renderLogs();
      }
    };

    console.warn = (...args: unknown[]) => {
      addToBuffer(`${COLORS.yellow}[WARN]${COLORS.reset} `, ...args);
      if (this.currentView === 'logs') {
        this.renderLogs();
      }
    };
  }

  private restoreConsole(): void {
    console.log = this.originalConsoleLog;
    console.error = this.originalConsoleError;
    console.warn = this.originalConsoleWarn;
  }

  private handleKeypress(_str: string, key: readline.Key): void {
    if (key.ctrl && key.name === 'c') {
      this.stop();
      process.exit(0);
    }

    switch (key.name) {
      case 'l':
        this.currentView = 'logs';
        this.renderLogs();
        break;
      case 'p':
        this.currentView = 'peers';
        this.renderPeers();
        break;
      case 'd':
        this.currentView = 'dag';
        this.renderDag();
        break;
      case 's':
        this.currentView = 'specs';
        this.renderSpecs();
        break;
      case 't':
        this.currentView = 'threads';
        this.renderThreads();
        break;
      case 'q':
        this.stop();
        process.exit(0);
        break;
      case 'escape':
        this.currentView = 'dashboard';
        this.renderDashboard();
        break;
      case 'up':
        if (this.currentView === 'threads') {
          const current = this.deps.getCryptoWorkers();
          const max = cpus().length;
          if (current < max) {
            this.deps.setCryptoWorkers(current + 1);
            this.renderThreads();
          }
        }
        break;
      case 'down':
        if (this.currentView === 'threads') {
          const current = this.deps.getCryptoWorkers();
          if (current > 1) {
            this.deps.setCryptoWorkers(current - 1);
            this.renderThreads();
          }
        }
        break;
    }
  }

  private clearScreen(): void {
    process.stdout.write('\x1b[2J\x1b[H');
  }

  private moveCursor(row: number, col: number): void {
    process.stdout.write(`\x1b[${row};${col}H`);
  }

  private calculateTps(): number {
    const currentTxCount = this.deps.consensus.getTotalTransactionsProcessed();
    const now = Date.now();
    const elapsed = (now - this.lastTpsCheck) / 1000;
    
    if (elapsed >= 1) {
      const txDiff = currentTxCount - this.lastTxCount;
      const tps = txDiff / elapsed;
      this.tpsHistory.push(tps);
      if (this.tpsHistory.length > 10) this.tpsHistory.shift();
      this.lastTxCount = currentTxCount;
      this.lastTpsCheck = now;
    }

    const avg = this.tpsHistory.length > 0 
      ? this.tpsHistory.reduce((a, b) => a + b, 0) / this.tpsHistory.length 
      : 0;
    return Math.round(avg * 10) / 10;
  }

  private renderDashboard(): void {
    this.clearScreen();
    const telemetry = this.deps.telemetry.collect();
    const dag = this.deps.consensus.getDAG();
    const tips = dag.getTips();
    const accountCount = this.deps.state.getAllAccounts().size;
    const checkpointHeight = this.deps.getCheckpointHeight();
    const pendingCount = this.deps.getPendingCount();
    const witnessedCount = this.deps.getWitnessedCount();
    const gasPriceData = this.deps.gas?.getCurrentGasPrice();
    const gasPrice = gasPriceData?.current ?? 0.001;
    const tps = this.calculateTps();
    const peerCount = this.deps.peerSync?.getPeerCount() ?? 0;
    const maxPeers = 50;

    const width = 65;
    const line = (char: string) => char.repeat(width - 2);

    const protoVer = this.deps.protocolVersion ? ` (proto ${this.deps.protocolVersion})` : '';
    const header = `${COLORS.cyan}${COLORS.bold}  RINKU NODE ${this.deps.version}${protoVer}${COLORS.reset}`;
    const uptime = `${COLORS.dim}Uptime: ${formatDuration(telemetry.processUptime)}${COLORS.reset}`;

    process.stdout.write(`${COLORS.cyan}╔${line('═')}╗${COLORS.reset}\n`);
    process.stdout.write(`${COLORS.cyan}║${COLORS.reset}${header}${' '.repeat(width - 30 - uptime.length + 20)}${uptime}${COLORS.cyan}║${COLORS.reset}\n`);
    process.stdout.write(`${COLORS.cyan}╠${line('═')}╣${COLORS.reset}\n`);

    const leftCol = 30;
    const rightCol = width - leftCol - 4;

    const rows = [
      [`${COLORS.green}DAG${COLORS.reset}`, `${COLORS.magenta}SYSTEM${COLORS.reset}`],
      [`├─ Nodes: ${dag.getAllNodes().length}`, `├─ CPU: ${telemetry.cpu.usage}%`],
      [`├─ Tips: ${tips.length}`, `├─ Heap: ${formatBytes(telemetry.memory.heapUsed)}`],
      [`├─ Accounts: ${accountCount}`, `├─ RSS: ${formatBytes(telemetry.memory.rss)}`],
      [`└─ Witnessed: ${witnessedCount}`, `└─ Data: ${formatBytes(telemetry.disk.dataDir)}`],
      ['', ''],
      [`${COLORS.yellow}CONSENSUS${COLORS.reset}`, `${COLORS.blue}NETWORK${COLORS.reset}`],
      [`├─ Checkpoint: #${checkpointHeight}`, `├─ Peers: ${peerCount}/${maxPeers}`],
      [`├─ Pending: ${pendingCount} txs`, `├─ In: ${formatBytes(telemetry.network.rateIn)}/s`],
      [`├─ Gas: ${gasPrice.toFixed(4)} RKU`, `├─ Out: ${formatBytes(telemetry.network.rateOut)}/s`],
      [`└─ TPS: ${tps}`, `└─ Gossip: ${this.deps.gossip ? 'active' : 'disabled'}`],
    ];

    for (const [left, right] of rows) {
      const leftClean = left.replace(/\x1b\[[0-9;]*m/g, '');
      const rightClean = right.replace(/\x1b\[[0-9;]*m/g, '');
      const leftPad = leftCol - leftClean.length;
      const rightPad = rightCol - rightClean.length;
      process.stdout.write(`${COLORS.cyan}║${COLORS.reset} ${left}${' '.repeat(Math.max(0, leftPad))}│ ${right}${' '.repeat(Math.max(0, rightPad))}${COLORS.cyan}║${COLORS.reset}\n`);
    }

    process.stdout.write(`${COLORS.cyan}╠${line('═')}╣${COLORS.reset}\n`);
    const menuItems = `${COLORS.dim}[L]${COLORS.reset} Logs  ${COLORS.dim}[P]${COLORS.reset} Peers  ${COLORS.dim}[D]${COLORS.reset} DAG  ${COLORS.dim}[S]${COLORS.reset} Specs  ${COLORS.dim}[T]${COLORS.reset} Threads  ${COLORS.dim}[Q]${COLORS.reset} Quit`;
    const menuClean = menuItems.replace(/\x1b\[[0-9;]*m/g, '');
    const menuPad = width - 3 - menuClean.length;
    process.stdout.write(`${COLORS.cyan}║${COLORS.reset} ${menuItems}${' '.repeat(Math.max(0, menuPad))}${COLORS.cyan}║${COLORS.reset}\n`);
    process.stdout.write(`${COLORS.cyan}╚${line('═')}╝${COLORS.reset}\n`);
  }

  private renderLogs(): void {
    this.clearScreen();
    const width = 80;
    const height = process.stdout.rows || 24;
    
    process.stdout.write(`${COLORS.cyan}${COLORS.bold}═══ LIVE LOGS ═══${COLORS.reset}  ${COLORS.dim}[ESC] Back to Dashboard${COLORS.reset}\n\n`);

    const displayLogs = this.logBuffer.slice(-(height - 4));
    for (const log of displayLogs) {
      process.stdout.write(log.slice(0, width) + '\n');
    }
  }

  private renderPeers(): void {
    this.clearScreen();
    process.stdout.write(`${COLORS.cyan}${COLORS.bold}═══ PEER CONNECTIONS ═══${COLORS.reset}  ${COLORS.dim}[ESC] Back to Dashboard${COLORS.reset}\n\n`);

    const peers = this.deps.peerSync?.getPeers() ?? [];
    
    if (peers.length === 0) {
      process.stdout.write(`${COLORS.dim}No peers connected${COLORS.reset}\n`);
      return;
    }

    process.stdout.write(`${COLORS.bold}  ID              URL                          Status${COLORS.reset}\n`);
    process.stdout.write(`${'─'.repeat(60)}\n`);

    for (const peer of peers) {
      const statusIcon = peer.status === 'online' ? `${COLORS.green}●${COLORS.reset}` : `${COLORS.red}○${COLORS.reset}`;
      const id = (peer.nodeId || 'unknown').slice(0, 12);
      const url = peer.url.slice(0, 30);
      process.stdout.write(`  ${id.padEnd(14)}  ${url.padEnd(30)}  ${statusIcon}\n`);
    }

    process.stdout.write(`\n${COLORS.dim}Total: ${peers.length} peers${COLORS.reset}\n`);
  }

  private renderDag(): void {
    this.clearScreen();
    process.stdout.write(`${COLORS.cyan}${COLORS.bold}═══ DAG DETAILS ═══${COLORS.reset}  ${COLORS.dim}[ESC] Back to Dashboard${COLORS.reset}\n\n`);

    const dag = this.deps.consensus.getDAG();
    const tips = dag.getTips();
    const nodes = dag.getAllNodes();

    process.stdout.write(`${COLORS.bold}Total Nodes:${COLORS.reset} ${nodes.length}\n`);
    process.stdout.write(`${COLORS.bold}Active Tips:${COLORS.reset} ${tips.length}\n\n`);

    process.stdout.write(`${COLORS.yellow}Recent Tips:${COLORS.reset}\n`);
    for (const tipHash of tips.slice(0, 10)) {
      const node = dag.getNode(tipHash);
      if (node) {
        const sender = node.tx.from?.slice(0, 12) || 'genesis';
        const weight = node.weight.toFixed(2);
        process.stdout.write(`  ${COLORS.dim}${tipHash.slice(0, 16)}...${COLORS.reset} from ${sender} w=${weight}\n`);
      }
    }

    if (tips.length > 10) {
      process.stdout.write(`  ${COLORS.dim}... and ${tips.length - 10} more${COLORS.reset}\n`);
    }
  }

  private renderSpecs(): void {
    this.clearScreen();
    process.stdout.write(`${COLORS.cyan}${COLORS.bold}═══ SYSTEM SPECIFICATIONS ═══${COLORS.reset}  ${COLORS.dim}[ESC] Back to Dashboard${COLORS.reset}\n\n`);

    const specs = this.deps.telemetry.getSpecs();
    const telemetry = this.deps.telemetry.collect();

    process.stdout.write(`${COLORS.bold}Hardware${COLORS.reset}\n`);
    process.stdout.write(`  CPU Model:    ${specs.cpu}\n`);
    process.stdout.write(`  CPU Cores:    ${specs.cores}\n`);
    process.stdout.write(`  Total RAM:    ${formatBytes(specs.totalRam)}\n`);
    process.stdout.write(`  Platform:     ${specs.platform} (${specs.arch})\n\n`);

    process.stdout.write(`${COLORS.bold}Process${COLORS.reset}\n`);
    process.stdout.write(`  Node ID:      ${this.deps.nodeId}\n`);
    process.stdout.write(`  Heap Used:    ${formatBytes(telemetry.memory.heapUsed)} / ${formatBytes(telemetry.memory.heapTotal)}\n`);
    process.stdout.write(`  RSS Memory:   ${formatBytes(telemetry.memory.rss)}\n`);
    process.stdout.write(`  System Free:  ${formatBytes(telemetry.memory.systemFree)}\n`);
    process.stdout.write(`  Uptime:       ${formatDuration(telemetry.processUptime)}\n\n`);

    process.stdout.write(`${COLORS.bold}Network${COLORS.reset}\n`);
    process.stdout.write(`  Total In:     ${formatBytes(telemetry.network.bytesIn)}\n`);
    process.stdout.write(`  Total Out:    ${formatBytes(telemetry.network.bytesOut)}\n`);
    process.stdout.write(`  Data Dir:     ${formatBytes(telemetry.disk.dataDir)}\n`);
  }

  private renderThreads(): void {
    this.clearScreen();
    process.stdout.write(`${COLORS.cyan}${COLORS.bold}═══ THREAD CONFIGURATION ═══${COLORS.reset}  ${COLORS.dim}[ESC] Back to Dashboard${COLORS.reset}\n\n`);

    const currentWorkers = this.deps.getCryptoWorkers();
    const maxWorkers = cpus().length;

    process.stdout.write(`${COLORS.bold}Crypto Worker Threads${COLORS.reset}\n\n`);
    
    const barWidth = 30;
    const filled = Math.round((currentWorkers / maxWorkers) * barWidth);
    const bar = `${COLORS.green}${'█'.repeat(filled)}${COLORS.dim}${'░'.repeat(barWidth - filled)}${COLORS.reset}`;
    
    process.stdout.write(`  ${bar} ${currentWorkers}/${maxWorkers}\n\n`);

    process.stdout.write(`${COLORS.dim}Use ↑/↓ arrows to adjust thread count${COLORS.reset}\n\n`);

    process.stdout.write(`${COLORS.yellow}Recommendations:${COLORS.reset}\n`);
    process.stdout.write(`  • Low traffic:     1-2 threads\n`);
    process.stdout.write(`  • Medium traffic:  ${Math.max(2, Math.floor(maxWorkers / 2))} threads\n`);
    process.stdout.write(`  • High traffic:    ${maxWorkers - 1} threads\n`);
    process.stdout.write(`  • Validator node:  ${maxWorkers} threads\n\n`);

    process.stdout.write(`${COLORS.dim}Note: Changes take effect immediately for new operations.${COLORS.reset}\n`);
    process.stdout.write(`${COLORS.dim}Set CRYPTO_WORKERS env var to persist across restarts.${COLORS.reset}\n`);
  }
}
