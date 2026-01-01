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
} from '../encoding.js';
import type { Transaction, ContractDeploy, ContractTransaction } from '../types.js';

describe('Encoding Module', () => {
  const mockTransaction: Transaction = {
    from: 'alice123',
    to: 'bob456',
    amount: 100,
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
  });
});
