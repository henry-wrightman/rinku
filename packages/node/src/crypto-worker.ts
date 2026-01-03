import { parentPort, workerData } from 'worker_threads';

const encoder = new TextEncoder();

function hexToArray(hex: string): Uint8Array {
  const matches = hex.match(/.{1,2}/g);
  if (!matches) return new Uint8Array(0);
  return new Uint8Array(matches.map(byte => parseInt(byte, 16)));
}

function arrayToHex(arr: Uint8Array): string {
  return Array.from(arr)
    .map(b => b.toString(16).padStart(2, '0'))
    .join('');
}

async function verifySignature(
  data: string,
  signature: string,
  publicKey: Uint8Array
): Promise<boolean> {
  try {
    const key = await crypto.subtle.importKey(
      'raw',
      publicKey,
      { name: 'ECDSA', namedCurve: 'P-256' },
      false,
      ['verify']
    );

    return await crypto.subtle.verify(
      { name: 'ECDSA', hash: 'SHA-256' },
      key,
      hexToArray(signature),
      encoder.encode(data)
    );
  } catch {
    return false;
  }
}

async function hashData(data: string): Promise<string> {
  const hashBuffer = await crypto.subtle.digest('SHA-256', encoder.encode(data));
  return arrayToHex(new Uint8Array(hashBuffer));
}

interface VerifyTask {
  id: number;
  type: 'verify';
  data: string;
  signature: string;
  publicKey: number[];
}

interface HashTask {
  id: number;
  type: 'hash';
  data: string;
}

interface BatchTask {
  id: number;
  type: 'batch_verify';
  items: Array<{
    data: string;
    signature: string;
    publicKey: number[];
  }>;
}

type Task = VerifyTask | HashTask | BatchTask;

parentPort?.on('message', async (task: Task) => {
  try {
    if (task.type === 'verify') {
      const result = await verifySignature(
        task.data,
        task.signature,
        new Uint8Array(task.publicKey)
      );
      parentPort?.postMessage({ id: task.id, result, error: null });
    } else if (task.type === 'hash') {
      const result = await hashData(task.data);
      parentPort?.postMessage({ id: task.id, result, error: null });
    } else if (task.type === 'batch_verify') {
      const results = await Promise.all(
        task.items.map(item =>
          verifySignature(item.data, item.signature, new Uint8Array(item.publicKey))
        )
      );
      parentPort?.postMessage({ id: task.id, result: results, error: null });
    }
  } catch (error: any) {
    parentPort?.postMessage({ id: task.id, result: null, error: error.message });
  }
});
