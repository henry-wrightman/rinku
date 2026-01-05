import { hash, hashTransaction, generateKeyPair, sign, verify, computeFingerprint, arrayToHex } from '../src/crypto.js';
import { getMerkleRoot, getTransactionMerkleRoot, getMerkleProof, verifyMerkleProof } from '../src/merkle.js';
import type { AccountState, Transaction } from '../src/types.js';
import * as fs from 'fs';
import * as path from 'path';

interface TestVectors {
  version: string;
  generated_at: string;
  hash: {
    description: string;
    vectors: Array<{
      input: string;
      expected: string;
    }>;
  };
  fingerprint: {
    description: string;
    vectors: Array<{
      public_key_hex: string;
      expected: string;
    }>;
  };
  ecdsa_p256: {
    description: string;
    key_format: {
      public_key: string;
      private_key: string;
    };
    signature_format: string;
    vectors: Array<{
      name: string;
      message: string;
      public_key_hex: string;
      private_key_hex: string;
      signature_hex: string;
      valid: boolean;
    }>;
  };
  merkle: {
    description: string;
    leaf_format: string;
    internal_node_format: string;
    odd_handling: string;
    vectors: Array<{
      name: string;
      accounts: Array<{
        fingerprint: string;
        balance: number;
        nonce: number;
      }>;
      expected_root: string;
      leaf_hashes: string[];
    }>;
  };
  transaction_hash: {
    description: string;
    vectors: Array<{
      name: string;
      tx: {
        from: string;
        to: string;
        amount: number;
        fee: number;
        nonce: number;
        tipUrls: string[];
        ts: number;
      };
      expected_hash: string;
    }>;
  };
}

async function generateTestVectors(): Promise<void> {
  console.log('Generating test vectors...');

  const vectors: TestVectors = {
    version: '1.0.0',
    generated_at: new Date().toISOString(),
    hash: {
      description: 'SHA-256 hash of UTF-8 encoded string, output as lowercase hex',
      vectors: [],
    },
    fingerprint: {
      description: 'First 40 hex chars of SHA-256(publicKey)',
      vectors: [],
    },
    ecdsa_p256: {
      description: 'ECDSA P-256 with SHA-256 hash',
      key_format: {
        public_key: '65 bytes uncompressed (0x04 prefix)',
        private_key: '32 bytes raw scalar (for Rust compatibility)',
      },
      signature_format: '64 bytes raw (r || s), output as lowercase hex',
      vectors: [],
    },
    merkle: {
      description: 'Binary Merkle tree with SHA-256, leaves sorted alphabetically by fingerprint',
      leaf_format: "SHA-256('{fingerprint}:{balance}:{nonce}')",
      internal_node_format: 'SHA-256(left_hash + right_hash)',
      odd_handling: 'Duplicate last node if odd count',
      vectors: [],
    },
    transaction_hash: {
      description: 'SHA-256 of JSON.stringify({from, to, amount, fee, nonce, tipUrls, ts})',
      vectors: [],
    },
  };

  console.log('Generating hash vectors...');
  const hashInputs = ['', 'Hello, Rinku!', 'test', 'abc', '{"key":"value"}'];
  for (const input of hashInputs) {
    const expected = await hash(input);
    vectors.hash.vectors.push({ input, expected });
  }

  console.log('Generating keypair and fingerprint vectors...');
  const kp = await generateKeyPair();
  vectors.fingerprint.vectors.push({
    public_key_hex: arrayToHex(kp.publicKey),
    expected: kp.fingerprint,
  });

  console.log('Generating ECDSA vectors...');
  const testMessages = ['Hello, Rinku!', 'test message', ''];
  for (const message of testMessages) {
    const testKp = await generateKeyPair();
    const sig = await sign(message, testKp.privateKey);
    const valid = await verify(message, sig, testKp.publicKey);

    vectors.ecdsa_p256.vectors.push({
      name: `sign_verify_${message.replace(/[^a-zA-Z0-9]/g, '_') || 'empty'}`,
      message,
      public_key_hex: arrayToHex(testKp.publicKey),
      private_key_hex: arrayToHex(testKp.privateKey),
      signature_hex: sig,
      valid,
    });
  }

  console.log('Generating Merkle tree vectors...');
  const merkleTestCases: Array<{
    name: string;
    accounts: Array<{ fingerprint: string; balance: number; nonce: number }>;
  }> = [
    {
      name: 'single_account',
      accounts: [{ fingerprint: 'account1', balance: 1000, nonce: 1 }],
    },
    {
      name: 'two_accounts',
      accounts: [
        { fingerprint: 'a', balance: 100, nonce: 1 },
        { fingerprint: 'b', balance: 200, nonce: 2 },
      ],
    },
    {
      name: 'three_accounts',
      accounts: [
        { fingerprint: 'alice', balance: 1000, nonce: 5 },
        { fingerprint: 'bob', balance: 2000, nonce: 3 },
        { fingerprint: 'charlie', balance: 500, nonce: 1 },
      ],
    },
    {
      name: 'four_accounts',
      accounts: [
        { fingerprint: 'a', balance: 100, nonce: 1 },
        { fingerprint: 'b', balance: 200, nonce: 2 },
        { fingerprint: 'c', balance: 300, nonce: 3 },
        { fingerprint: 'd', balance: 400, nonce: 4 },
      ],
    },
  ];

  for (const testCase of merkleTestCases) {
    const accountsMap = new Map<string, AccountState>();
    const leafHashes: string[] = [];

    for (const acc of testCase.accounts) {
      accountsMap.set(acc.fingerprint, {
        fingerprint: acc.fingerprint,
        balance: acc.balance,
        nonce: acc.nonce,
        firstTxTimestamp: 0,
      });
    }

    const sortedEntries = Array.from(accountsMap.entries()).sort((a, b) =>
      a[0].localeCompare(b[0])
    );
    for (const [fp, state] of sortedEntries) {
      const leafHash = await hash(`${fp}:${state.balance}:${state.nonce}`);
      leafHashes.push(leafHash);
    }

    const root = await getMerkleRoot(accountsMap);
    vectors.merkle.vectors.push({
      name: testCase.name,
      accounts: testCase.accounts,
      expected_root: root,
      leaf_hashes: leafHashes,
    });
  }

  console.log('Generating transaction hash vectors...');
  const txTestCases = [
    {
      name: 'simple_transfer',
      tx: {
        from: 'alice_fingerprint_40chars_________________',
        to: 'bob_fingerprint_40chars___________________',
        amount: 100,
        fee: 0.001,
        nonce: 1,
        tipUrls: [] as string[],
        ts: 1700000000000,
      },
    },
    {
      name: 'with_tips',
      tx: {
        from: 'sender123456789012345678901234567890',
        to: 'receiver12345678901234567890123456789',
        amount: 50.5,
        fee: 0.01,
        nonce: 42,
        tipUrls: ['/tx/h/abc123', '/tx/h/def456'],
        ts: 1700000001000,
      },
    },
    {
      name: 'zero_amount',
      tx: {
        from: 'from_address_40_chars_____________________',
        to: 'to_address_40_chars_______________________',
        amount: 0,
        fee: 0,
        nonce: 0,
        tipUrls: [] as string[],
        ts: 0,
      },
    },
  ];

  for (const testCase of txTestCases) {
    const fullTx: Transaction = {
      ...testCase.tx,
      sig: '',
    };
    const txHash = await hashTransaction(fullTx);
    vectors.transaction_hash.vectors.push({
      name: testCase.name,
      tx: testCase.tx,
      expected_hash: txHash,
    });
  }

  const outputPath = path.join(import.meta.dirname || '.', '../test-vectors.json');
  fs.writeFileSync(outputPath, JSON.stringify(vectors, null, 2));
  console.log(`Test vectors written to ${outputPath}`);
  console.log(`Total vectors: ${
    vectors.hash.vectors.length +
    vectors.fingerprint.vectors.length +
    vectors.ecdsa_p256.vectors.length +
    vectors.merkle.vectors.length +
    vectors.transaction_hash.vectors.length
  }`);
}

generateTestVectors().catch(console.error);
