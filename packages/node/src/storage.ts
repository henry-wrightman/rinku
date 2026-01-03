import { promises as fs } from 'fs';
import { existsSync, mkdirSync } from 'fs';
import path from 'path';

export interface NodeSnapshot {
  version: number;
  timestamp: number;
  state: {
    accounts: [string, any][];
    merkleRoot: string;
  };
  dag: {
    nodes: any[];
    tips: string[];
  };
  publicKeys: [string, number[]][];
  rewards?: object;
  checkpoints?: object;
  gas?: object;
  tokenomics?: {
    emission?: object;
    slashing?: object;
  };
  finality?: object;
  contracts?: {
    contracts: { id: string; contract: object }[];
    executionHistory: [string, object[]][];
    stateTrie: { storage: [string, unknown][] };
    receiptsTrie: { receipts: [string, unknown][] };
  };
  validatorKeys?: object;
  proofSlashing?: object;
}

export class Storage {
  private dataDir: string;
  private snapshotPath: string;

  constructor(dataDir: string = '.rinku-data') {
    this.dataDir = dataDir;
    this.snapshotPath = path.join(dataDir, 'node.json');
    
    if (!existsSync(dataDir)) {
      mkdirSync(dataDir, { recursive: true });
    }
  }

  async save(snapshot: NodeSnapshot): Promise<void> {
    if (!existsSync(this.dataDir)) {
      mkdirSync(this.dataDir, { recursive: true });
    }
    const tempPath = this.snapshotPath + '.tmp';
    await fs.writeFile(tempPath, JSON.stringify(snapshot));
    await fs.rename(tempPath, this.snapshotPath);
  }

  async load(): Promise<NodeSnapshot | null> {
    try {
      if (!existsSync(this.snapshotPath)) {
        return null;
      }
      const data = await fs.readFile(this.snapshotPath, 'utf-8');
      return JSON.parse(data) as NodeSnapshot;
    } catch {
      return null;
    }
  }

  async exists(): Promise<boolean> {
    return existsSync(this.snapshotPath);
  }

  getDataDir(): string {
    return this.dataDir;
  }
}
