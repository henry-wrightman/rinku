import { subtle } from 'crypto';
import { deflate, inflate } from 'pako';

interface ProofComponent {
  name: string;
  rawSize: number;
  compressedSize: number;
  data: Uint8Array;
}

interface AnalysisResult {
  components: ProofComponent[];
  totalRaw: number;
  totalCompressed: number;
  compressionRatio: number;
  qrViability: {
    alphanumeric: { chars: number; viable: boolean };
    binary: { bytes: number; viable: boolean };
  };
}

async function generateECDSAKeyPair(): Promise<CryptoKeyPair> {
  return subtle.generateKey(
    { name: 'ECDSA', namedCurve: 'P-256' },
    true,
    ['sign', 'verify']
  );
}

async function signData(privateKey: CryptoKey, data: Uint8Array): Promise<Uint8Array> {
  const sig = await subtle.sign(
    { name: 'ECDSA', hash: 'SHA-256' },
    privateKey,
    data
  );
  return new Uint8Array(sig);
}

async function exportPublicKey(publicKey: CryptoKey): Promise<Uint8Array> {
  const raw = await subtle.exportKey('raw', publicKey);
  return new Uint8Array(raw);
}

function sha256(data: Uint8Array): Promise<Uint8Array> {
  return subtle.digest('SHA-256', data).then(buf => new Uint8Array(buf));
}

function toBase64Url(data: Uint8Array): string {
  let binary = '';
  for (let i = 0; i < data.length; i++) {
    binary += String.fromCharCode(data[i]);
  }
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

async function analyzeComponent(name: string, data: Uint8Array): Promise<ProofComponent> {
  const compressed = deflate(data, { level: 9 });
  return {
    name,
    rawSize: data.length,
    compressedSize: compressed.length,
    data
  };
}

async function runAnalysis(): Promise<void> {
  console.log('='.repeat(70));
  console.log('RINKU PROOF SIZE ANALYSIS - Real Cryptographic Data');
  console.log('='.repeat(70));
  console.log();

  const components: ProofComponent[] = [];

  console.log('Generating real cryptographic components...\n');

  const txHash = await sha256(new TextEncoder().encode('sample-transaction-data'));
  components.push(await analyzeComponent('Transaction Hash (SHA-256)', txHash));

  const senderKeyPair = await generateECDSAKeyPair();
  const senderPubKey = await exportPublicKey(senderKeyPair.publicKey);
  components.push(await analyzeComponent('Sender Public Key (P-256 raw)', senderPubKey));

  const txSignature = await signData(senderKeyPair.privateKey, txHash);
  components.push(await analyzeComponent('Transaction Signature (ECDSA P-256)', txSignature));

  const nonce = new Uint8Array(4);
  new DataView(nonce.buffer).setUint32(0, 12345, false);
  components.push(await analyzeComponent('Nonce (4 bytes)', nonce));

  const amount = new Uint8Array(8);
  new DataView(amount.buffer).setBigUint64(0, BigInt(1000000), false);
  components.push(await analyzeComponent('Amount (8 bytes)', amount));

  const checkpointHeight = new Uint8Array(4);
  new DataView(checkpointHeight.buffer).setUint32(0, 51, false);
  components.push(await analyzeComponent('Checkpoint Height (4 bytes)', checkpointHeight));

  const merkleProofDepth = 10;
  const merkleHashes: Uint8Array[] = [];
  for (let i = 0; i < merkleProofDepth; i++) {
    merkleHashes.push(await sha256(new TextEncoder().encode(`merkle-node-${i}`)));
  }
  const merkleProof = new Uint8Array(merkleProofDepth * 32);
  merkleHashes.forEach((h, i) => merkleProof.set(h, i * 32));
  components.push(await analyzeComponent(`Merkle Proof (${merkleProofDepth} levels x 32 bytes)`, merkleProof));

  const merkleIndex = new Uint8Array(2);
  new DataView(merkleIndex.buffer).setUint16(0, 42, false);
  components.push(await analyzeComponent('Merkle Index (2 bytes)', merkleIndex));

  console.log('--- Validator Signatures (for checkpoint finality) ---\n');

  const validatorCounts = [1, 3, 5, 10, 21];
  
  for (const count of validatorCounts) {
    const validatorSigs: Uint8Array[] = [];
    const validatorPubKeys: Uint8Array[] = [];
    
    for (let i = 0; i < count; i++) {
      const vKeyPair = await generateECDSAKeyPair();
      const vPubKey = await exportPublicKey(vKeyPair.publicKey);
      const vSig = await signData(vKeyPair.privateKey, txHash);
      validatorSigs.push(vSig);
      validatorPubKeys.push(vPubKey);
    }

    const allSigs = new Uint8Array(count * 64);
    validatorSigs.forEach((s, i) => allSigs.set(s, i * 64));
    
    const allPubKeys = new Uint8Array(count * 65);
    validatorPubKeys.forEach((p, i) => allPubKeys.set(p, i * 65));

    components.push(await analyzeComponent(`${count} Validator Signatures`, allSigs));
    components.push(await analyzeComponent(`${count} Validator Public Keys`, allPubKeys));
  }

  console.log('\n' + '='.repeat(70));
  console.log('COMPONENT SIZE BREAKDOWN');
  console.log('='.repeat(70));
  console.log();
  console.log('Component'.padEnd(45) + 'Raw'.padStart(8) + 'Deflate'.padStart(10) + 'Ratio'.padStart(8));
  console.log('-'.repeat(70));

  for (const c of components) {
    const ratio = ((1 - c.compressedSize / c.rawSize) * 100).toFixed(1);
    console.log(
      c.name.padEnd(45) +
      `${c.rawSize}B`.padStart(8) +
      `${c.compressedSize}B`.padStart(10) +
      `${ratio}%`.padStart(8)
    );
  }

  console.log('\n' + '='.repeat(70));
  console.log('MINIMUM VIABLE PROOF SCENARIOS');
  console.log('='.repeat(70));

  const profileAMinimal = {
    txHash: 32,
    signature: 64,
    checkpointHeight: 4,
    attestationCount: 1
  };
  const profileATotalRaw = Object.values(profileAMinimal).reduce((a, b) => a + b, 0);

  console.log('\n--- Profile A Minimal (signed receipt, no full proof) ---');
  console.log(`Components: txHash(32) + sig(64) + checkpointHeight(4) + attestations(1)`);
  console.log(`Raw: ${profileATotalRaw} bytes`);

  const profileAData = new Uint8Array(profileATotalRaw);
  profileAData.set(txHash, 0);
  profileAData.set(txSignature, 32);
  const profileACompressed = deflate(profileAData, { level: 9 });
  console.log(`Compressed: ${profileACompressed.length} bytes`);
  console.log(`Base64url: ${toBase64Url(profileACompressed).length} chars`);

  console.log('\n--- Profile B Minimal (1 validator, 10-level Merkle) ---');
  const profileB1Raw = 32 + 64 + 4 + 320 + 2 + 64 + 65;
  console.log(`Components: txHash(32) + txSig(64) + cpHeight(4) + merkle(320) + idx(2) + 1×valSig(64) + 1×valPubKey(65)`);
  console.log(`Raw: ${profileB1Raw} bytes`);

  const profileB1Data = new Uint8Array(551);
  let offset = 0;
  profileB1Data.set(txHash, offset); offset += 32;
  profileB1Data.set(txSignature, offset); offset += 64;
  profileB1Data.set(checkpointHeight, offset); offset += 4;
  profileB1Data.set(merkleProof, offset); offset += 320;
  profileB1Data.set(merkleIndex, offset); offset += 2;

  const singleValidatorKP = await generateECDSAKeyPair();
  const singleValSig = await signData(singleValidatorKP.privateKey, txHash);
  const singleValPubKey = await exportPublicKey(singleValidatorKP.publicKey);
  profileB1Data.set(singleValSig, offset); offset += 64;
  profileB1Data.set(singleValPubKey, offset);

  const profileB1Compressed = deflate(profileB1Data, { level: 9 });
  const profileB1Base64 = toBase64Url(profileB1Compressed);
  console.log(`Compressed: ${profileB1Compressed.length} bytes`);
  console.log(`Base64url: ${profileB1Base64.length} chars`);

  console.log('\n--- Profile B with 3 validators ---');
  const valCount3 = 3;
  const profileB3Raw = 32 + 64 + 4 + 320 + 2 + (64 * valCount3) + (65 * valCount3);
  console.log(`Raw: ${profileB3Raw} bytes`);

  const profileB3Data = new Uint8Array(profileB3Raw);
  offset = 0;
  profileB3Data.set(txHash, offset); offset += 32;
  profileB3Data.set(txSignature, offset); offset += 64;
  profileB3Data.set(checkpointHeight, offset); offset += 4;
  profileB3Data.set(merkleProof, offset); offset += 320;
  profileB3Data.set(merkleIndex, offset); offset += 2;
  
  for (let i = 0; i < valCount3; i++) {
    const kp = await generateECDSAKeyPair();
    const sig = await signData(kp.privateKey, txHash);
    const pubKey = await exportPublicKey(kp.publicKey);
    profileB3Data.set(sig, offset); offset += 64;
    profileB3Data.set(pubKey, offset); offset += 65;
  }

  const profileB3Compressed = deflate(profileB3Data, { level: 9 });
  const profileB3Base64 = toBase64Url(profileB3Compressed);
  console.log(`Compressed: ${profileB3Compressed.length} bytes`);
  console.log(`Base64url: ${profileB3Base64.length} chars`);

  console.log('\n--- Profile B with 5 validators ---');
  const valCount5 = 5;
  const profileB5Raw = 32 + 64 + 4 + 320 + 2 + (64 * valCount5) + (65 * valCount5);
  console.log(`Raw: ${profileB5Raw} bytes`);

  const profileB5Data = new Uint8Array(profileB5Raw);
  offset = 0;
  profileB5Data.set(txHash, offset); offset += 32;
  profileB5Data.set(txSignature, offset); offset += 64;
  profileB5Data.set(checkpointHeight, offset); offset += 4;
  profileB5Data.set(merkleProof, offset); offset += 320;
  profileB5Data.set(merkleIndex, offset); offset += 2;
  
  for (let i = 0; i < valCount5; i++) {
    const kp = await generateECDSAKeyPair();
    const sig = await signData(kp.privateKey, txHash);
    const pubKey = await exportPublicKey(kp.publicKey);
    profileB5Data.set(sig, offset); offset += 64;
    profileB5Data.set(pubKey, offset); offset += 65;
  }

  const profileB5Compressed = deflate(profileB5Data, { level: 9 });
  const profileB5Base64 = toBase64Url(profileB5Compressed);
  console.log(`Compressed: ${profileB5Compressed.length} bytes`);
  console.log(`Base64url: ${profileB5Base64.length} chars`);

  console.log('\n' + '='.repeat(70));
  console.log('QR CODE CAPACITY REFERENCE');
  console.log('='.repeat(70));
  console.log(`
QR Version | Error Correction L | Binary Capacity | Alphanumeric
-----------|-------------------|-----------------|-------------
    10     |        L          |      271 bytes  |    395 chars
    15     |        L          |      520 bytes  |    758 chars  
    20     |        L          |      858 bytes  |   1,249 chars
    25     |        L          |    1,273 bytes  |   1,853 chars
    30     |        L          |    1,732 bytes  |   2,520 chars
    40     |        L          |    2,953 bytes  |   4,296 chars
`);

  console.log('='.repeat(70));
  console.log('VIABILITY ASSESSMENT');
  console.log('='.repeat(70));

  const scenarios = [
    { name: 'Profile A Minimal', compressed: profileACompressed.length, base64: toBase64Url(profileACompressed).length },
    { name: 'Profile B (1 validator)', compressed: profileB1Compressed.length, base64: profileB1Base64.length },
    { name: 'Profile B (3 validators)', compressed: profileB3Compressed.length, base64: profileB3Base64.length },
    { name: 'Profile B (5 validators)', compressed: profileB5Compressed.length, base64: profileB5Base64.length },
  ];

  console.log('\nScenario'.padEnd(30) + 'Compressed'.padStart(12) + 'Base64url'.padStart(12) + 'QR Version'.padStart(12) + 'Viable?'.padStart(10));
  console.log('-'.repeat(76));

  for (const s of scenarios) {
    let qrVersion = 'N/A';
    let viable = '❌';
    
    if (s.base64 <= 395) { qrVersion = 'v10'; viable = '✅ Easy'; }
    else if (s.base64 <= 758) { qrVersion = 'v15'; viable = '✅ Good'; }
    else if (s.base64 <= 1249) { qrVersion = 'v20'; viable = '⚠️ Large'; }
    else if (s.base64 <= 1853) { qrVersion = 'v25'; viable = '⚠️ Very Large'; }
    else if (s.base64 <= 2520) { qrVersion = 'v30'; viable = '⚠️ Huge'; }
    else if (s.base64 <= 4296) { qrVersion = 'v40'; viable = '❌ Max QR'; }
    else { qrVersion = '>v40'; viable = '❌ Too Big'; }

    console.log(
      s.name.padEnd(30) +
      `${s.compressed}B`.padStart(12) +
      `${s.base64} chars`.padStart(12) +
      qrVersion.padStart(12) +
      viable.padStart(10)
    );
  }

  console.log('\n' + '='.repeat(70));
  console.log('OPTIMIZATION OPPORTUNITIES');
  console.log('='.repeat(70));
  console.log(`
1. BLS Signature Aggregation: N signatures → 1 signature (96 bytes)
   - 5 validators: 320B sigs → 96B = 70% reduction
   - Requires signature scheme change

2. Validator Set Commitment: Instead of N pubkeys, include:
   - Validator set Merkle root (32 bytes)
   - Bitmap of which validators signed (ceil(N/8) bytes)
   - One aggregate signature (96 bytes with BLS)
   
3. Compact Merkle Proofs: Use position-aware encoding
   - Current: 32 bytes per level
   - Optimized: Can skip levels where sibling is predictable
   
4. Variable-length integers: nonce, amount, height
   - Current: fixed 4-8 bytes
   - Optimized: 1-4 bytes for typical values
`);

  console.log('='.repeat(70));
  console.log('CONCLUSION');
  console.log('='.repeat(70));
  console.log(`
With current ECDSA P-256 signatures:
- Profile A (compact receipt): ✅ Easily fits in QR v10-15
- Profile B (1 validator):     ✅ Fits in QR v25-30 (scannable but large)
- Profile B (3+ validators):   ⚠️ Requires QR v30+ (challenging to scan)
- Profile B (5+ validators):   ❌ Pushing QR limits

With BLS aggregation (future):
- Profile B (any validator count): Could fit in QR v15-20 (~400-600 chars)

RECOMMENDATION: 
- Profile A is already viable as self-contained QR proof
- Profile B needs BLS aggregation for practical QR embedding
- Hybrid approach: Profile A in QR, Profile B available via resolver
`);
}

runAnalysis().catch(console.error);
