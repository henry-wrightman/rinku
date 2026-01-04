import { describe, it, expect } from 'vitest';
import {
  base64urlEncode,
  base64urlDecode,
  encodeTransaction,
  decodeTransaction,
  createTransactionURL,
  parseTransactionURL,
  encodeContractDeploy,
  decodeContractDeploy,
  createContractURL,
  parseContractURL,
  encodeContractTransaction,
  decodeContractTransaction,
  isURLSafe,
  chunkWasmCode,
  assembleWasmFromChunks,
  getURLType,
  getURLTypeExtended,
  encodeSelfCrawlableBundle,
  decodeSelfCrawlableBundle,
  createSelfCrawlableURL,
  parseSelfCrawlableURL,
  verifySelfCrawlableBundle,
  encodeContractReceiptProof,
  decodeContractReceiptProof,
  createContractReceiptURL,
  parseContractReceiptURL,
  encodeContractReceiptProofB,
  decodeContractReceiptProofB,
  createContractReceiptURLB,
  parseContractReceiptURLB,
  verifyContractReceiptProof,
  verifyReceiptMerkleProof,
  type ContractReceiptProofA,
  type ContractReceiptProofB,
} from '../encoding.js';
import type { Transaction, ContractDeploy, ContractTransaction, SelfCrawlableBundle, ContractReceipt } from '../types.js';
export {};

describe('Encoding Module', () => {
  const mockTransaction: Transaction = {
    from: 'alice123',
    to: 'bob456',
    amount: 100,
    fee: 0.01,
    nonce: 1,
    tipUrls: ['/tx/parent1', '/tx/parent2'],
    sig: 'signature123',
    ts: 1700000000000,
  };

  describe('Base64URL Encoding', () => {
    it('should encode and decode Uint8Array', () => {
      const data = new Uint8Array([1, 2, 3, 4, 5]);
      const encoded = base64urlEncode(data);
      const decoded = base64urlDecode(encoded);
      expect(Array.from(decoded)).toEqual(Array.from(data));
    });

    it('should handle empty data', () => {
      const data = new Uint8Array([]);
      const encoded = base64urlEncode(data);
      const decoded = base64urlDecode(encoded);
      expect(decoded.length).toBe(0);
    });

    it('should produce URL-safe output', () => {
      const data = new Uint8Array([255, 254, 253, 252]);
      const encoded = base64urlEncode(data);
      expect(encoded).not.toContain('+');
      expect(encoded).not.toContain('/');
      expect(encoded).not.toContain('=');
    });

    it('should ignore invalid characters in decode', () => {
      const decoded = base64urlDecode('AQID!@#$BA');
      expect(decoded.length).toBeGreaterThan(0);
    });
  });

  describe('Transaction Encoding', () => {
    it('should encode and decode transactions', () => {
      const encoded = encodeTransaction(mockTransaction);
      const decoded = decodeTransaction(encoded);
      expect(decoded.from).toBe(mockTransaction.from);
      expect(decoded.to).toBe(mockTransaction.to);
      expect(decoded.amount).toBe(mockTransaction.amount);
      expect(decoded.nonce).toBe(mockTransaction.nonce);
      expect(decoded.tipUrls).toEqual(mockTransaction.tipUrls);
    });

    it('should produce encoded output', () => {
      const encoded = encodeTransaction(mockTransaction);
      expect(encoded.length).toBeGreaterThan(0);
      expect(typeof encoded).toBe('string');
    });

    it('should handle transactions with empty tipUrls', () => {
      const tx = { ...mockTransaction, tipUrls: [] };
      const encoded = encodeTransaction(tx);
      const decoded = decodeTransaction(encoded);
      expect(decoded.tipUrls).toEqual([]);
    });
  });

  describe('URL Encoding', () => {
    it('should convert transactions to URLs', () => {
      const url = createTransactionURL(mockTransaction);
      expect(url.path).toMatch(/^\/tx\//);
      expect(url.payload).toBeDefined();
    });

    it('should parse transaction URLs', () => {
      const url = createTransactionURL(mockTransaction);
      const parsed = parseTransactionURL(url.path);
      expect(parsed).toBeDefined();
      expect(parsed!.from).toBe(mockTransaction.from);
    });

    it('should return null for invalid URLs', () => {
      const parsed = parseTransactionURL('/invalid/url');
      expect(parsed).toBeNull();
    });

    it('should return null for malformed payloads', () => {
      const parsed = parseTransactionURL('/tx/!!!invalid!!!');
      expect(parsed).toBeNull();
    });
  });

  describe('Contract Encoding', () => {
    const mockContractDeploy: ContractDeploy = {
      type: 'deploy',
      contractId: 'token_001',
      creator: 'alice123',
      wasmBase64: 'AGFzbQEAAAA=',
      initState: { totalSupply: 1000000 },
      tipUrls: ['/tx/parent1'],
      sig: 'deploysig',
      ts: 1700000000000,
    };

    it('should encode and decode contract deployment', () => {
      const encoded = encodeContractDeploy(mockContractDeploy);
      const decoded = decodeContractDeploy(encoded);
      expect(decoded.contractId).toBe(mockContractDeploy.contractId);
      expect(decoded.creator).toBe(mockContractDeploy.creator);
      expect(decoded.wasmBase64).toBe(mockContractDeploy.wasmBase64);
    });

    it('should create contract URLs', () => {
      const url = createContractURL(mockContractDeploy);
      expect(url.path).toMatch(/^\/sc\//);
      expect(url.payload).toBeDefined();
    });

    it('should parse contract URLs', () => {
      const url = createContractURL(mockContractDeploy);
      const parsed = parseContractURL(url.path);
      expect(parsed).toBeDefined();
      expect(parsed!.contractId).toBe(mockContractDeploy.contractId);
    });

    it('should return null for invalid contract URLs', () => {
      expect(parseContractURL('/invalid/url')).toBeNull();
      expect(parseContractURL('/sc/!!!invalid!!!')).toBeNull();
    });
  });

  describe('Contract Transaction Encoding', () => {
    const mockContractTx: ContractTransaction = {
      hash: 'txhash123',
      from: 'alice',
      to: 'contract_001',
      amount: 0,
      fee: 0.01,
      nonce: 1,
      tipUrls: [],
      sig: 'sig',
      ts: Date.now(),
      contract: {
        action: 'call',
        contractId: 'token_001',
        entrypoint: 'transfer',
        input: { to: 'bob', amount: 100 },
        preStateHash: 'abc123',
        postStateHash: 'def456',
      },
    };

    it('should encode and decode contract transactions', () => {
      const encoded = encodeContractTransaction(mockContractTx);
      const decoded = decodeContractTransaction(encoded);
      expect(decoded.contract?.contractId).toBe('token_001');
      expect(decoded.contract?.entrypoint).toBe('transfer');
    });
  });

  describe('URL Safety', () => {
    it('should validate URL length', () => {
      expect(isURLSafe('/tx/short')).toBe(true);
      expect(isURLSafe('x'.repeat(2000))).toBe(false);
    });
  });

  describe('WASM Chunking', () => {
    it('should chunk large WASM code', () => {
      const wasmBase64 = 'A'.repeat(3000);
      const chunks = chunkWasmCode(wasmBase64, 'contract_001');
      expect(chunks.length).toBe(3);
      expect(chunks[0]).toContain('/sc/chunk/contract_001/0/');
      expect(chunks[1]).toContain('/sc/chunk/contract_001/1/');
    });

    it('should reassemble WASM from chunks', () => {
      const original = 'ABCDEFGHIJ'.repeat(200);
      const chunks = chunkWasmCode(original, 'test_contract');
      const reassembled = assembleWasmFromChunks(chunks);
      expect(reassembled).toBe(original);
    });

    it('should throw for invalid chunk URL', () => {
      expect(() => assembleWasmFromChunks(['/invalid/url'])).toThrow('Invalid chunk URL');
    });
  });

  describe('URL Type Detection', () => {
    it('should detect transaction URLs', () => {
      expect(getURLType('/tx/payload123')).toBe('tx');
    });

    it('should detect contract URLs', () => {
      expect(getURLType('/sc/payload123')).toBe('sc');
    });

    it('should detect chunk URLs', () => {
      expect(getURLType('/sc/chunk/contract/0/data')).toBe('sc-chunk');
    });

    it('should return unknown for other URLs', () => {
      expect(getURLType('/other/path')).toBe('unknown');
      expect(getURLType('http://example.com')).toBe('unknown');
    });

    it('should detect hash-based tx URLs', () => {
      expect(getURLType('/tx/h/abc123')).toBe('tx');
    });

    it('should detect txp URLs', () => {
      expect(getURLType('/txp/payload123')).toBe('txp');
    });
  });

  describe('Extended URL Type Detection', () => {
    it('should detect receipt proof URLs', () => {
      expect(getURLTypeExtended('/rxp/payload123')).toBe('rxp');
      expect(getURLTypeExtended('/rxpb/payload123')).toBe('rxpb');
    });

    it('should detect all standard URL types', () => {
      expect(getURLTypeExtended('/tx/payload')).toBe('tx');
      expect(getURLTypeExtended('/tx/h/hash')).toBe('tx');
      expect(getURLTypeExtended('/txp/payload')).toBe('txp');
      expect(getURLTypeExtended('/sc/payload')).toBe('sc');
      expect(getURLTypeExtended('/sc/chunk/id/0/data')).toBe('sc-chunk');
    });
  });

  describe('Self-Crawlable Bundle Encoding', () => {
    const mockBundle: SelfCrawlableBundle = {
      tx: {
        from: 'alice',
        to: 'bob',
        amount: 100,
        fee: 0.01,
        nonce: 1,
        tipUrls: [],
        sig: 'sig123',
        ts: Date.now(),
      },
      hash: 'txhash123',
      parents: [],
    };

    it('should encode and decode self-crawlable bundle', () => {
      const encoded = encodeSelfCrawlableBundle(mockBundle);
      const decoded = decodeSelfCrawlableBundle(encoded);
      expect(decoded.hash).toBe(mockBundle.hash);
      expect(decoded.tx.from).toBe(mockBundle.tx.from);
    });

    it('should create self-crawlable URL', () => {
      const url = createSelfCrawlableURL(mockBundle);
      expect(url.path).toMatch(/^\/txp\//);
      expect(url.payload).toBeDefined();
    });

    it('should parse self-crawlable URL', () => {
      const url = createSelfCrawlableURL(mockBundle);
      const parsed = parseSelfCrawlableURL(url.path);
      expect(parsed).toBeDefined();
      expect(parsed!.hash).toBe(mockBundle.hash);
    });

    it('should return null for invalid self-crawlable URL', () => {
      expect(parseSelfCrawlableURL('/invalid/url')).toBeNull();
      expect(parseSelfCrawlableURL('/txp/!!!invalid!!!')).toBeNull();
    });

    it('should handle bundle with parents', () => {
      const parentBundle: SelfCrawlableBundle = {
        tx: { ...mockBundle.tx, nonce: 0 },
        hash: 'parenthash',
        parents: [],
      };
      const bundleWithParents: SelfCrawlableBundle = {
        ...mockBundle,
        parents: [parentBundle],
      };
      const encoded = encodeSelfCrawlableBundle(bundleWithParents);
      const decoded = decodeSelfCrawlableBundle(encoded);
      expect(decoded.parents.length).toBe(1);
      expect(decoded.parents[0].hash).toBe('parenthash');
    });

    it('should handle bundle with checkpoint anchor', () => {
      const bundleWithAnchor: SelfCrawlableBundle = {
        ...mockBundle,
        checkpointAnchor: {
          checkpointId: 'cp123',
          merkleRoot: 'root123',
          height: 100,
          signatureCount: 5,
        },
      };
      const encoded = encodeSelfCrawlableBundle(bundleWithAnchor);
      const decoded = decodeSelfCrawlableBundle(encoded);
      expect(decoded.checkpointAnchor?.checkpointId).toBe('cp123');
    });
  });

  describe('Self-Crawlable Bundle Verification', () => {
    const validBundle: SelfCrawlableBundle = {
      tx: {
        from: 'alice',
        to: 'bob',
        amount: 100,
        fee: 0.01,
        nonce: 1,
        tipUrls: [],
        sig: 'sig123',
        ts: Date.now(),
      },
      hash: 'txhash123',
      parents: [],
    };

    it('should verify valid bundle', () => {
      const result = verifySelfCrawlableBundle(validBundle);
      expect(result.valid).toBe(true);
      expect(result.errors).toEqual([]);
      expect(result.transactionCount).toBe(1);
    });

    it('should detect missing tx', () => {
      const invalidBundle = { hash: 'hash', parents: [] } as any;
      const result = verifySelfCrawlableBundle(invalidBundle);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Bundle missing tx field');
    });

    it('should detect missing hash', () => {
      const invalidBundle = { tx: validBundle.tx, parents: [] } as any;
      const result = verifySelfCrawlableBundle(invalidBundle);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Bundle missing hash field');
    });

    it('should detect invalid amount', () => {
      const invalidBundle: SelfCrawlableBundle = {
        ...validBundle,
        tx: { ...validBundle.tx, amount: 0 },
      };
      const result = verifySelfCrawlableBundle(invalidBundle);
      expect(result.valid).toBe(false);
    });

    it('should detect missing signature', () => {
      const invalidBundle: SelfCrawlableBundle = {
        ...validBundle,
        tx: { ...validBundle.tx, sig: '' },
      };
      const result = verifySelfCrawlableBundle(invalidBundle);
      expect(result.valid).toBe(false);
    });

    it('should count nested transactions', () => {
      const parent1: SelfCrawlableBundle = { ...validBundle, hash: 'p1', parents: [] };
      const parent2: SelfCrawlableBundle = { ...validBundle, hash: 'p2', parents: [] };
      const bundleWithParents: SelfCrawlableBundle = {
        ...validBundle,
        parents: [parent1, parent2],
      };
      const result = verifySelfCrawlableBundle(bundleWithParents);
      expect(result.transactionCount).toBe(3);
      expect(result.maxDepth).toBe(1);
    });

    it('should detect duplicate transactions', () => {
      const parent: SelfCrawlableBundle = { ...validBundle, hash: 'dup', parents: [] };
      const bundleWithDupes: SelfCrawlableBundle = {
        ...validBundle,
        parents: [parent, parent],
      };
      const result = verifySelfCrawlableBundle(bundleWithDupes);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Duplicate'))).toBe(true);
    });

    it('should report checkpoint anchor presence', () => {
      const bundleWithAnchor: SelfCrawlableBundle = {
        ...validBundle,
        checkpointAnchor: {
          checkpointId: 'cp123',
          merkleRoot: 'root123',
          height: 100,
          signatureCount: 5,
        },
      };
      const result = verifySelfCrawlableBundle(bundleWithAnchor);
      expect(result.hasCheckpointAnchor).toBe(true);
      expect(result.checkpointId).toBe('cp123');
    });

    it('should detect invalid checkpoint anchor', () => {
      const bundleWithBadAnchor: SelfCrawlableBundle = {
        ...validBundle,
        checkpointAnchor: {
          checkpointId: '',
          merkleRoot: 'root',
          height: 100,
          signatureCount: 0,
        },
      };
      const result = verifySelfCrawlableBundle(bundleWithBadAnchor);
      expect(result.valid).toBe(false);
    });
  });

  describe('Contract Receipt Proof Encoding (Profile A)', () => {
    const FIXED_TS = 1700000000000;
    const mockReceipt = {
      callId: 'call123',
      txHash: 'txhash123',
      contractId: 'contract123',
      entrypoint: 'transfer',
      caller: 'alice',
      preStateRoot: 'pre123',
      postStateRoot: 'post123',
      effectsHash: 'effects123',
      status: 'success' as const,
      gasUsed: 1000,
      gasLimit: 2000,
      eventsHash: 'events123',
      eventCount: 0,
      executedAt: FIXED_TS,
    };

    const mockProofA: ContractReceiptProofA = {
      receipt: mockReceipt,
      tx: {
        from: 'alice',
        to: 'contract123',
        amount: 0,
        fee: 0.01,
        nonce: 1,
        tipUrls: [],
        sig: 'sig123',
        ts: FIXED_TS,
      },
      txHash: 'txhash123',
      checkpointAnchor: {
        checkpointId: 'cp123',
        height: 100,
        stateRoot: 'state123',
        receiptRoot: 'receipt123',
        merkleRoot: 'merkle123',
        signatureCount: 5,
      },
    };

    it('should encode and decode Profile A proof', () => {
      const encoded = encodeContractReceiptProof(mockProofA);
      const decoded = decodeContractReceiptProof(encoded);
      expect(decoded.receipt.callId).toBe(mockProofA.receipt.callId);
      expect(decoded.txHash).toBe(mockProofA.txHash);
    });

    it('should create Profile A URL', () => {
      const url = createContractReceiptURL(mockProofA);
      expect(url.path).toMatch(/^\/rxp\//);
    });

    it('should parse Profile A URL', () => {
      const url = createContractReceiptURL(mockProofA);
      const parsed = parseContractReceiptURL(url.path);
      expect(parsed).toBeDefined();
      expect(parsed!.receipt.callId).toBe(mockProofA.receipt.callId);
    });

    it('should return null for invalid Profile A URL', () => {
      expect(parseContractReceiptURL('/invalid')).toBeNull();
      expect(parseContractReceiptURL('/rxp/!!!invalid!!!')).toBeNull();
    });
  });

  describe('Contract Receipt Proof Encoding (Profile B)', () => {
    const FIXED_TS = 1700000000000;
    const mockFullReceipt: ContractReceipt = {
      callId: 'call123',
      txHash: 'txhash123',
      contractId: 'contract123',
      entrypoint: 'transfer',
      caller: 'alice',
      preStateRoot: 'pre123',
      postStateRoot: 'post123',
      effectsHash: 'effects123',
      status: 'success',
      gasUsed: 1000,
      gasLimit: 2000,
      eventsHash: 'events123',
      eventCount: 1,
      executedAt: FIXED_TS,
      events: [{ contractId: 'contract123', eventName: 'Transfer', data: { from: 'alice', to: 'bob', amount: 100 }, index: 0 }],
    };

    const mockProofB: ContractReceiptProofB = {
      receipt: mockFullReceipt,
      tx: {
        from: 'alice',
        to: 'contract123',
        amount: 0,
        fee: 0.01,
        nonce: 1,
        tipUrls: [],
        sig: 'sig123',
        ts: FIXED_TS,
      },
      txHash: 'txhash123',
      checkpointAnchor: {
        checkpointId: 'cp123',
        height: 100,
        stateRoot: 'state123',
        receiptRoot: 'receipt123',
        merkleRoot: 'merkle123',
        signatureCount: 5,
      },
      witness: {
        touchedKeys: [{ contractId: 'contract123', key: 'balance', preValue: 100, postValue: 50 }],
        merkleProofs: [],
      },
      validatorSignatures: [
        { validator: 'val1', signature: 'sig1', weight: 100 },
      ],
      receiptMerkleProof: {
        proof: ['sibling1', 'sibling2'],
        index: 0,
        receiptRoot: 'receipt123',
      },
    };

    it('should encode and decode Profile B proof', () => {
      const encoded = encodeContractReceiptProofB(mockProofB);
      const decoded = decodeContractReceiptProofB(encoded);
      expect(decoded.receipt.callId).toBe(mockProofB.receipt.callId);
      expect(decoded.witness.touchedKeys.length).toBe(1);
      expect(decoded.validatorSignatures.length).toBe(1);
    });

    it('should create Profile B URL', () => {
      const url = createContractReceiptURLB(mockProofB);
      expect(url.path).toMatch(/^\/rxpb\//);
    });

    it('should parse Profile B URL', () => {
      const url = createContractReceiptURLB(mockProofB);
      const parsed = parseContractReceiptURLB(url.path);
      expect(parsed).toBeDefined();
      expect(parsed!.witness.touchedKeys.length).toBe(1);
    });

    it('should return null for invalid Profile B URL', () => {
      expect(parseContractReceiptURLB('/invalid')).toBeNull();
      expect(parseContractReceiptURLB('/rxpb/!!!invalid!!!')).toBeNull();
    });
  });

  describe('Contract Receipt Proof Verification', () => {
    const FIXED_TS = 1700000000000;
    const validProofA: ContractReceiptProofA = {
      receipt: {
        callId: 'call123',
        txHash: 'txhash123',
        contractId: 'contract123',
        entrypoint: 'transfer',
        caller: 'alice',
        preStateRoot: 'pre123',
        postStateRoot: 'post123',
        effectsHash: 'effects123',
        status: 'success' as const,
        gasUsed: 1000,
        gasLimit: 2000,
        eventsHash: 'events123',
        eventCount: 0,
        executedAt: FIXED_TS,
      },
      tx: {
        from: 'alice',
        to: 'contract123',
        amount: 0,
        fee: 0.01,
        nonce: 1,
        tipUrls: [],
        sig: 'sig123',
        ts: FIXED_TS,
      },
      txHash: 'txhash123',
      checkpointAnchor: {
        checkpointId: 'cp123',
        height: 100,
        stateRoot: 'state123',
        receiptRoot: 'receipt123',
        merkleRoot: 'merkle123',
        signatureCount: 5,
      },
    };

    it('should verify valid Profile A proof', () => {
      const result = verifyContractReceiptProof(validProofA);
      expect(result.valid).toBe(true);
      expect(result.profile).toBe('A');
      expect(result.errors).toEqual([]);
    });

    it('should detect missing receipt', () => {
      const invalidProof = { ...validProofA, receipt: undefined } as any;
      const result = verifyContractReceiptProof(invalidProof);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Missing receipt');
    });

    it('should detect missing receipt fields', () => {
      const invalidReceipt = { ...validProofA.receipt, callId: '' };
      const invalidProof = { ...validProofA, receipt: invalidReceipt };
      const result = verifyContractReceiptProof(invalidProof);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Receipt missing callId');
    });

    it('should detect missing transaction', () => {
      const invalidProof = { ...validProofA, tx: undefined } as any;
      const result = verifyContractReceiptProof(invalidProof);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Missing transaction');
    });

    it('should detect missing checkpoint anchor', () => {
      const invalidProof = { ...validProofA, checkpointAnchor: undefined } as any;
      const result = verifyContractReceiptProof(invalidProof);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Missing checkpoint anchor');
    });

    it('should detect missing anchor fields', () => {
      const invalidAnchor = { ...validProofA.checkpointAnchor, checkpointId: '' };
      const invalidProof = { ...validProofA, checkpointAnchor: invalidAnchor };
      const result = verifyContractReceiptProof(invalidProof);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Anchor missing checkpointId');
    });

    it('should identify Profile B proofs', () => {
      const proofB: ContractReceiptProofB = {
        ...validProofA,
        receipt: { ...validProofA.receipt, events: [] },
        witness: { touchedKeys: [], merkleProofs: [] },
        validatorSignatures: [{ validator: 'v1', signature: 's1', weight: 100 }],
        receiptMerkleProof: { proof: [], index: 0, receiptRoot: 'root' },
      };
      const result = verifyContractReceiptProof(proofB);
      expect(result.profile).toBe('B');
      expect(result.hasWitness).toBe(true);
      expect(result.hasValidatorSignatures).toBe(true);
      expect(result.signatureCount).toBe(1);
    });

    it('should detect missing Profile B witness', () => {
      const proofB = {
        ...validProofA,
        receipt: { ...validProofA.receipt, events: [] },
        witness: undefined,
        validatorSignatures: [{ validator: 'v1', signature: 's1', weight: 100 }],
        receiptMerkleProof: { proof: [], index: 0, receiptRoot: 'root' },
      } as any;
      const result = verifyContractReceiptProof(proofB);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Profile B missing witness');
    });

    it('should detect missing Profile B signatures', () => {
      const proofB = {
        ...validProofA,
        receipt: { ...validProofA.receipt, events: [] },
        witness: { touchedKeys: [], merkleProofs: [] },
        validatorSignatures: [],
        receiptMerkleProof: { proof: [], index: 0, receiptRoot: 'root' },
      } as any;
      const result = verifyContractReceiptProof(proofB);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Profile B missing validator signatures');
    });
  });

  describe('Receipt Merkle Proof Verification', () => {
    const FIXED_TS = 1700000000000;
    
    it('should verify valid Merkle proof', async () => {
      const { hash } = await import('../crypto.js');
      const receipt: ContractReceipt = {
        callId: 'call1',
        txHash: 'tx1',
        contractId: 'c1',
        entrypoint: 'fn',
        caller: 'alice',
        preStateRoot: 'pre',
        postStateRoot: 'post',
        effectsHash: 'e1',
        status: 'success',
        gasUsed: 100,
        gasLimit: 200,
        eventsHash: 'ev1',
        eventCount: 0,
        executedAt: FIXED_TS,
        events: [],
      };

      const leafData = JSON.stringify({ id: 'call1', receipt });
      const leafHash = await hash(leafData);
      
      const valid = await verifyReceiptMerkleProof('call1', receipt, [], 0, leafHash);
      expect(valid).toBe(true);
    });

    it('should reject invalid Merkle proof', async () => {
      const receipt: ContractReceipt = {
        callId: 'call1',
        txHash: 'tx1',
        contractId: 'c1',
        entrypoint: 'fn',
        caller: 'alice',
        preStateRoot: 'pre',
        postStateRoot: 'post',
        effectsHash: 'e1',
        status: 'success',
        gasUsed: 100,
        gasLimit: 200,
        eventsHash: 'ev1',
        eventCount: 0,
        executedAt: FIXED_TS,
        events: [],
      };

      const valid = await verifyReceiptMerkleProof('call1', receipt, [], 0, 'wrongroot');
      expect(valid).toBe(false);
    });

    it('should handle proof with siblings', async () => {
      const { hash } = await import('../crypto.js');
      const receipt: ContractReceipt = {
        callId: 'call1',
        txHash: 'tx1',
        contractId: 'c1',
        entrypoint: 'fn',
        caller: 'alice',
        preStateRoot: 'pre',
        postStateRoot: 'post',
        effectsHash: 'e1',
        status: 'success',
        gasUsed: 100,
        gasLimit: 200,
        eventsHash: 'ev1',
        eventCount: 0,
        executedAt: FIXED_TS,
        events: [],
      };

      const leafData = JSON.stringify({ id: 'call1', receipt });
      const leafHash = await hash(leafData);
      const sibling = await hash('sibling');
      const expectedRoot = await hash(leafHash + sibling);
      
      const valid = await verifyReceiptMerkleProof('call1', receipt, [sibling], 0, expectedRoot);
      expect(valid).toBe(true);
    });

    it('should handle odd index in proof', async () => {
      const { hash } = await import('../crypto.js');
      const receipt: ContractReceipt = {
        callId: 'call1',
        txHash: 'tx1',
        contractId: 'c1',
        entrypoint: 'fn',
        caller: 'alice',
        preStateRoot: 'pre',
        postStateRoot: 'post',
        effectsHash: 'e1',
        status: 'success',
        gasUsed: 100,
        gasLimit: 200,
        eventsHash: 'ev1',
        eventCount: 0,
        executedAt: FIXED_TS,
        events: [],
      };

      const leafData = JSON.stringify({ id: 'call1', receipt });
      const leafHash = await hash(leafData);
      const sibling = await hash('sibling');
      const expectedRoot = await hash(sibling + leafHash);
      
      const valid = await verifyReceiptMerkleProof('call1', receipt, [sibling], 1, expectedRoot);
      expect(valid).toBe(true);
    });
  });

  describe('WASM Chunk Assembly', () => {
    it('should chunk and reassemble WASM code', () => {
      const wasmBase64 = 'A'.repeat(3500);
      const chunks = chunkWasmCode(wasmBase64, 'contract123');
      expect(chunks.length).toBe(4);
      const reassembled = assembleWasmFromChunks(chunks);
      expect(reassembled).toBe(wasmBase64);
    });

    it('should throw for invalid chunk URL format', () => {
      expect(() => assembleWasmFromChunks(['/invalid/url'])).toThrow('Invalid chunk URL');
    });

    it('should sort chunks by index', () => {
      const chunks = [
        '/sc/chunk/c1/2/CC',
        '/sc/chunk/c1/0/AA',
        '/sc/chunk/c1/1/BB',
      ];
      const reassembled = assembleWasmFromChunks(chunks);
      expect(reassembled).toBe('AABBCC');
    });

    it('should handle single chunk', () => {
      const chunks = chunkWasmCode('short', 'c1');
      expect(chunks.length).toBe(1);
      expect(assembleWasmFromChunks(chunks)).toBe('short');
    });
  });

  describe('Malformed Payload Handling', () => {
    it('should return null for corrupted transaction URL', () => {
      const result = parseTransactionURL('/tx/!!invalid!!');
      expect(result).toBeNull();
    });

    it('should return null for corrupted contract URL', () => {
      const result = parseContractURL('/sc/!!invalid!!');
      expect(result).toBeNull();
    });

    it('should return null for corrupted self-crawlable URL', () => {
      const result = parseSelfCrawlableURL('/txp/!!invalid!!');
      expect(result).toBeNull();
    });

    it('should return null for corrupted receipt URL', () => {
      const result = parseContractReceiptURL('/rxp/!!invalid!!');
      expect(result).toBeNull();
    });

    it('should return null for corrupted receipt URL B', () => {
      const result = parseContractReceiptURLB('/rxpb/!!invalid!!');
      expect(result).toBeNull();
    });

    it('should handle empty payloads gracefully', () => {
      expect(parseTransactionURL('/tx/')).toBeNull();
      expect(parseContractURL('/sc/')).toBeNull();
      expect(parseSelfCrawlableURL('/txp/')).toBeNull();
    });
  });

  describe('URL Type Detection', () => {
    it('should detect transaction URLs', () => {
      expect(getURLType('/tx/payload')).toBe('tx');
      expect(getURLType('/tx/h/abc123')).toBe('tx');
    });

    it('should detect self-crawlable URLs', () => {
      expect(getURLType('/txp/payload')).toBe('txp');
    });

    it('should detect contract URLs', () => {
      expect(getURLType('/sc/payload')).toBe('sc');
    });

    it('should detect chunk URLs', () => {
      expect(getURLType('/sc/chunk/c1/0/data')).toBe('sc-chunk');
    });

    it('should return unknown for unrecognized URLs', () => {
      expect(getURLType('/unknown/path')).toBe('unknown');
      expect(getURLType('')).toBe('unknown');
    });

    it('should detect extended URL types', () => {
      expect(getURLTypeExtended('/rxp/payload')).toBe('rxp');
      expect(getURLTypeExtended('/rxpb/payload')).toBe('rxpb');
      expect(getURLTypeExtended('/tx/payload')).toBe('tx');
      expect(getURLTypeExtended('/unknown')).toBe('unknown');
    });
  });

  describe('URL Safety', () => {
    it('should detect safe URLs', () => {
      expect(isURLSafe('/tx/short')).toBe(true);
    });

    it('should detect unsafe URLs', () => {
      const longPayload = 'A'.repeat(2000);
      expect(isURLSafe(`/tx/${longPayload}`)).toBe(false);
    });
  });

  describe('Bundle Verification Edge Cases', () => {
    it('should detect missing tx field', () => {
      const bundle = { hash: 'h1', parents: [] } as any;
      const result = verifySelfCrawlableBundle(bundle);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Bundle missing tx field');
    });

    it('should detect missing hash field', () => {
      const bundle = { tx: { from: 'a', to: 'b', amount: 1, sig: 's' }, parents: [] } as any;
      const result = verifySelfCrawlableBundle(bundle);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Bundle missing hash field');
    });

    it('should detect invalid amount', () => {
      const bundle = {
        tx: { from: 'a', to: 'b', amount: -1, sig: 's' },
        hash: 'h1',
        parents: []
      } as any;
      const result = verifySelfCrawlableBundle(bundle);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('invalid amount'))).toBe(true);
    });

    it('should detect missing signature', () => {
      const bundle = {
        tx: { from: 'a', to: 'b', amount: 1 },
        hash: 'h1',
        parents: []
      } as any;
      const result = verifySelfCrawlableBundle(bundle);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('missing signature'))).toBe(true);
    });

    it('should validate checkpoint anchor fields', () => {
      const bundle = {
        tx: { from: 'a', to: 'b', amount: 1, sig: 's' },
        hash: 'h1',
        parents: [],
        checkpointAnchor: { checkpointId: '', merkleRoot: '' }
      } as any;
      const result = verifySelfCrawlableBundle(bundle);
      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Invalid checkpoint anchor'))).toBe(true);
    });

    it('should count transactions and depth', () => {
      const parent: any = {
        tx: { from: 'a', to: 'b', amount: 1, sig: 's' },
        hash: 'p1',
        parents: []
      };
      const bundle: any = {
        tx: { from: 'c', to: 'd', amount: 2, sig: 's2' },
        hash: 'h1',
        parents: [parent]
      };
      const result = verifySelfCrawlableBundle(bundle);
      expect(result.transactionCount).toBe(2);
      expect(result.maxDepth).toBe(1);
    });
  });

  describe('Receipt Proof Verification Edge Cases', () => {
    it('should detect missing receipt', () => {
      const proof = { tx: {}, txHash: 'h1', checkpointAnchor: { checkpointId: 'cp1', stateRoot: 's', receiptRoot: 'r' } } as any;
      const result = verifyContractReceiptProof(proof);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Missing receipt');
    });

    it('should detect missing transaction', () => {
      const proof = {
        receipt: { callId: 'c1', txHash: 't1', contractId: 'cn1', status: 'success', effectsHash: 'e1', eventsHash: 'ev1' },
        txHash: 'h1',
        checkpointAnchor: { checkpointId: 'cp1', stateRoot: 's', receiptRoot: 'r' }
      } as any;
      const result = verifyContractReceiptProof(proof);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Missing transaction');
    });

    it('should detect missing receipt merkle proof in Profile B', () => {
      const proof = {
        receipt: { callId: 'c1', txHash: 't1', contractId: 'cn1', status: 'success', effectsHash: 'e1', eventsHash: 'ev1' },
        tx: { from: 'a', to: 'b' },
        txHash: 'h1',
        checkpointAnchor: { checkpointId: 'cp1', stateRoot: 's', receiptRoot: 'r' },
        witness: { touchedKeys: [], merkleProofs: [] },
        validatorSignatures: [{ validator: 'v1', signature: 's1', weight: 100 }]
      } as any;
      const result = verifyContractReceiptProof(proof);
      expect(result.valid).toBe(false);
      expect(result.errors).toContain('Profile B missing receipt Merkle proof');
    });
  });

  describe('Encoding Round-trip Edge Cases', () => {
    it('should handle transaction with zero fee', () => {
      const tx = { ...mockTransaction, fee: 0 };
      const encoded = encodeTransaction(tx);
      const decoded = decodeTransaction(encoded);
      expect(decoded.fee).toBe(0);
    });

    it('should handle transaction with large nonce', () => {
      const tx = { ...mockTransaction, nonce: 999999999 };
      const encoded = encodeTransaction(tx);
      const decoded = decodeTransaction(encoded);
      expect(decoded.nonce).toBe(999999999);
    });

    it('should handle transaction with special characters in addresses', () => {
      const tx = { ...mockTransaction, from: 'addr_with-special.chars', to: 'another_addr-123' };
      const encoded = encodeTransaction(tx);
      const decoded = decodeTransaction(encoded);
      expect(decoded.from).toBe('addr_with-special.chars');
      expect(decoded.to).toBe('another_addr-123');
    });

    it('should handle contract deploy with empty wasm', () => {
      const deploy: ContractDeploy = {
        type: 'deploy',
        contractId: 'c1',
        creator: 'alice',
        wasmBase64: '',
        initState: {},
        tipUrls: [],
        sig: 'sig',
        ts: 1700000000000
      };
      const encoded = encodeContractDeploy(deploy);
      const decoded = decodeContractDeploy(encoded);
      expect(decoded.wasmBase64).toBe('');
    });

    it('should handle contract transaction encoding', () => {
      const tx = {
        from: 'alice',
        to: 'contract1',
        amount: 0,
        fee: 0.01,
        nonce: 1,
        tipUrls: [],
        sig: 'sig',
        ts: 1700000000000,
        hash: 'txhash123',
        contract: {
          action: 'call',
          contractId: 'c1',
          entrypoint: 'transfer',
          input: { to: 'bob', amount: 100 },
          preStateHash: 'pre',
          postStateHash: 'post'
        }
      } as ContractTransaction;
      const encoded = encodeContractTransaction(tx);
      const decoded = decodeContractTransaction(encoded);
      expect(decoded.contract?.entrypoint).toBe('transfer');
    });
  });
});
