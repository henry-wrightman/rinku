import { Worker } from 'worker_threads';
import { cpus } from 'os';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';
import { existsSync } from 'fs';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

function findWorkerPath(): string {
  const distPath = join(__dirname, 'crypto-worker.js');
  if (existsSync(distPath)) return distPath;
  
  const devDistPath = join(__dirname, '..', 'dist', 'crypto-worker.js');
  if (existsSync(devDistPath)) return devDistPath;
  
  const srcDistPath = join(__dirname, '..', '..', 'dist', 'crypto-worker.js');
  if (existsSync(srcDistPath)) return srcDistPath;
  
  return distPath;
}

interface PendingTask {
  resolve: (result: any) => void;
  reject: (error: Error) => void;
}

interface WorkerInfo {
  worker: Worker;
  busy: boolean;
  currentTaskId: number | null;
  taskQueue: Array<{ task: any; pending: PendingTask }>;
}

export class CryptoPool {
  private workers: WorkerInfo[] = [];
  private taskId = 0;
  private pendingTasks = new Map<number, PendingTask>();
  private roundRobin = 0;
  private batchQueue: Array<{
    data: string;
    signature: string;
    publicKey: Uint8Array;
    resolve: (result: boolean) => void;
    reject: (error: Error) => void;
  }> = [];
  private batchTimeout: NodeJS.Timeout | null = null;
  private batchSize = 50;
  private batchDelayMs = 5;

  constructor(numWorkers?: number) {
    const workerCount = numWorkers || Math.max(2, cpus().length - 1);
    
    for (let i = 0; i < workerCount; i++) {
      this.createWorker();
    }
  }

  private createWorker(): void {
    const workerPath = findWorkerPath();
    const worker = new Worker(workerPath);
    
    const workerInfo: WorkerInfo = {
      worker,
      busy: false,
      currentTaskId: null,
      taskQueue: []
    };

    worker.on('message', (msg: { id: number; result: any; error: string | null }) => {
      const pending = this.pendingTasks.get(msg.id);
      if (pending) {
        this.pendingTasks.delete(msg.id);
        if (msg.error) {
          pending.reject(new Error(msg.error));
        } else {
          pending.resolve(msg.result);
        }
      }
      workerInfo.busy = false;
      workerInfo.currentTaskId = null;
      this.processQueue(workerInfo);
    });

    worker.on('error', (error) => {
      console.error('Crypto worker error:', error);
    });

    this.workers.push(workerInfo);
  }

  private processQueue(workerInfo: WorkerInfo): void {
    if (workerInfo.taskQueue.length > 0 && !workerInfo.busy) {
      const { task, pending } = workerInfo.taskQueue.shift()!;
      workerInfo.busy = true;
      workerInfo.currentTaskId = task.id;
      this.pendingTasks.set(task.id, pending);
      workerInfo.worker.postMessage(task);
    }
  }

  private getNextWorker(): WorkerInfo {
    const worker = this.workers[this.roundRobin];
    this.roundRobin = (this.roundRobin + 1) % this.workers.length;
    return worker;
  }

  async verify(data: string, signature: string, publicKey: Uint8Array): Promise<boolean> {
    return new Promise((resolve, reject) => {
      this.batchQueue.push({ data, signature, publicKey, resolve, reject });
      
      if (this.batchQueue.length >= this.batchSize) {
        this.flushBatch();
      } else if (!this.batchTimeout) {
        this.batchTimeout = setTimeout(() => this.flushBatch(), this.batchDelayMs);
      }
    });
  }

  private flushBatch(): void {
    if (this.batchTimeout) {
      clearTimeout(this.batchTimeout);
      this.batchTimeout = null;
    }

    if (this.batchQueue.length === 0) return;

    const batch = this.batchQueue.splice(0, this.batchSize);
    const taskId = this.taskId++;
    const worker = this.getNextWorker();

    const task = {
      id: taskId,
      type: 'batch_verify' as const,
      items: batch.map(b => ({
        data: b.data,
        signature: b.signature,
        publicKey: Array.from(b.publicKey)
      }))
    };

    const pending: PendingTask = {
      resolve: (results: boolean[]) => {
        this.pendingTasks.delete(taskId);
        for (let i = 0; i < batch.length; i++) {
          batch[i].resolve(results[i]);
        }
      },
      reject: (error: Error) => {
        this.pendingTasks.delete(taskId);
        for (const item of batch) {
          item.reject(error);
        }
      }
    };

    if (worker.busy) {
      worker.taskQueue.push({ task, pending });
    } else {
      worker.busy = true;
      worker.currentTaskId = taskId;
      this.pendingTasks.set(taskId, pending);
      worker.worker.postMessage(task);
    }

    if (this.batchQueue.length > 0) {
      this.flushBatch();
    }
  }

  async verifySingle(data: string, signature: string, publicKey: Uint8Array): Promise<boolean> {
    const taskId = this.taskId++;
    const worker = this.getNextWorker();

    const task = {
      id: taskId,
      type: 'verify' as const,
      data,
      signature,
      publicKey: Array.from(publicKey)
    };

    return new Promise((resolve, reject) => {
      const pending: PendingTask = { resolve, reject };

      if (worker.busy) {
        worker.taskQueue.push({ task, pending });
      } else {
        worker.busy = true;
        worker.currentTaskId = taskId;
        this.pendingTasks.set(taskId, pending);
        worker.worker.postMessage(task);
      }
    });
  }

  async hash(data: string): Promise<string> {
    const taskId = this.taskId++;
    const worker = this.getNextWorker();

    const task = {
      id: taskId,
      type: 'hash' as const,
      data
    };

    return new Promise((resolve, reject) => {
      const pending: PendingTask = { resolve, reject };

      if (worker.busy) {
        worker.taskQueue.push({ task, pending });
      } else {
        worker.busy = true;
        worker.currentTaskId = taskId;
        this.pendingTasks.set(taskId, pending);
        worker.worker.postMessage(task);
      }
    });
  }

  async verifyBatch(
    items: Array<{ data: string; signature: string; publicKey: Uint8Array }>
  ): Promise<boolean[]> {
    const taskId = this.taskId++;
    const worker = this.getNextWorker();

    const task = {
      id: taskId,
      type: 'batch_verify' as const,
      items: items.map(item => ({
        data: item.data,
        signature: item.signature,
        publicKey: Array.from(item.publicKey)
      }))
    };

    return new Promise((resolve, reject) => {
      const pending: PendingTask = { resolve, reject };

      if (worker.busy) {
        worker.taskQueue.push({ task, pending });
      } else {
        worker.busy = true;
        worker.currentTaskId = taskId;
        this.pendingTasks.set(taskId, pending);
        worker.worker.postMessage(task);
      }
    });
  }

  getStats(): { workers: number; pending: number; queued: number } {
    return {
      workers: this.workers.length,
      pending: this.pendingTasks.size,
      queued: this.workers.reduce((sum, w) => sum + w.taskQueue.length, 0) + this.batchQueue.length
    };
  }

  getWorkerCount(): number {
    return this.workers.length;
  }

  async resize(targetCount: number): Promise<void> {
    const currentCount = this.workers.length;
    const maxCount = cpus().length;
    const newCount = Math.max(1, Math.min(targetCount, maxCount));

    if (newCount === currentCount) return;

    if (newCount > currentCount) {
      for (let i = currentCount; i < newCount; i++) {
        this.createWorker();
      }
    } else {
      const toRemove = currentCount - newCount;
      const workersToRemove: WorkerInfo[] = [];
      
      for (let i = 0; i < toRemove; i++) {
        const workerInfo = this.workers.pop();
        if (workerInfo) {
          workersToRemove.push(workerInfo);
        }
      }
      
      this.roundRobin = 0;
      
      for (const workerInfo of workersToRemove) {
        for (const { task, pending } of workerInfo.taskQueue) {
          if (this.workers.length > 0) {
            const remainingWorker = this.getNextWorker();
            if (remainingWorker.busy) {
              remainingWorker.taskQueue.push({ task, pending });
            } else {
              remainingWorker.busy = true;
              remainingWorker.currentTaskId = task.id;
              this.pendingTasks.set(task.id, pending);
              remainingWorker.worker.postMessage(task);
            }
          } else {
            pending.reject(new Error('No workers available'));
          }
        }
        
        if (workerInfo.busy && workerInfo.currentTaskId !== null) {
          const inFlightTaskId = workerInfo.currentTaskId;
          await new Promise<void>((resolve) => {
            const onComplete = () => {
              workerInfo.worker.removeListener('message', onComplete);
              resolve();
            };
            workerInfo.worker.once('message', onComplete);
            
            setTimeout(() => {
              workerInfo.worker.removeListener('message', onComplete);
              const pending = this.pendingTasks.get(inFlightTaskId);
              if (pending) {
                pending.reject(new Error('Worker terminated during resize'));
                this.pendingTasks.delete(inFlightTaskId);
              }
              resolve();
            }, 2000);
          });
        }
        
        await workerInfo.worker.terminate();
      }
    }
  }

  async shutdown(): Promise<void> {
    if (this.batchTimeout) {
      clearTimeout(this.batchTimeout);
    }
    
    await Promise.all(this.workers.map(w => w.worker.terminate()));
    this.workers = [];
  }
}

let globalPool: CryptoPool | null = null;

export function getCryptoPool(): CryptoPool {
  if (!globalPool) {
    globalPool = new CryptoPool();
  }
  return globalPool;
}

export function initCryptoPool(numWorkers?: number): CryptoPool {
  if (globalPool) {
    globalPool.shutdown();
  }
  globalPool = new CryptoPool(numWorkers);
  return globalPool;
}
