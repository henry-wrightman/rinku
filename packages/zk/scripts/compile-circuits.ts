import { execSync, spawn } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const ZK_ROOT = path.resolve(__dirname, '..');
const WORKSPACE_ROOT = path.resolve(ZK_ROOT, '../..');
const BUILD_DIR = path.join(ZK_ROOT, 'build');
const CIRCUITS_DIR = path.join(ZK_ROOT, 'circuits');

const CIRCUIT_NAME = 'rinku_private_proof';
const PTAU_URL = 'https://storage.googleapis.com/zkevm/ptau/powersOfTau28_hez_final_14.ptau';
const PTAU_FILE = 'powersOfTau28_hez_final_14.ptau';

async function downloadPtau(): Promise<string> {
  const ptauPath = path.join(BUILD_DIR, PTAU_FILE);
  
  if (fs.existsSync(ptauPath)) {
    console.log(`✓ Powers of Tau file already exists: ${PTAU_FILE}`);
    return ptauPath;
  }
  
  console.log(`⬇ Downloading Powers of Tau ceremony file...`);
  console.log(`  Source: ${PTAU_URL}`);
  
  try {
    execSync(`curl -L -o "${ptauPath}" "${PTAU_URL}"`, { stdio: 'inherit' });
    console.log(`✓ Downloaded: ${PTAU_FILE}`);
    return ptauPath;
  } catch (error) {
    console.error('✗ Failed to download Powers of Tau file');
    throw error;
  }
}

async function compileCircuit(): Promise<void> {
  const circuitPath = path.join(CIRCUITS_DIR, `${CIRCUIT_NAME}.circom`);
  const outputDir = path.join(BUILD_DIR, CIRCUIT_NAME);
  
  if (!fs.existsSync(outputDir)) {
    fs.mkdirSync(outputDir, { recursive: true });
  }
  
  console.log(`\n⚙ Compiling circuit: ${CIRCUIT_NAME}.circom`);
  
  try {
    const nodeModulesPath = path.join(WORKSPACE_ROOT, 'node_modules');
    execSync(
      `circom "${circuitPath}" --r1cs --wasm --sym -o "${outputDir}" -l "${nodeModulesPath}"`,
      { stdio: 'inherit', cwd: ZK_ROOT }
    );
    console.log(`✓ Circuit compiled successfully`);
  } catch (error) {
    console.error('✗ Circuit compilation failed');
    throw error;
  }
}

async function generateZkey(ptauPath: string): Promise<void> {
  const outputDir = path.join(BUILD_DIR, CIRCUIT_NAME);
  const r1csPath = path.join(outputDir, `${CIRCUIT_NAME}.r1cs`);
  const zkey0Path = path.join(outputDir, `${CIRCUIT_NAME}_0.zkey`);
  const zkeyPath = path.join(outputDir, `${CIRCUIT_NAME}.zkey`);
  const vkeyPath = path.join(outputDir, 'verification_key.json');
  
  console.log(`\n🔐 Generating proving key (zkey)...`);
  
  try {
    execSync(
      `npx snarkjs groth16 setup "${r1csPath}" "${ptauPath}" "${zkey0Path}"`,
      { stdio: 'inherit', cwd: ZK_ROOT }
    );
    console.log(`✓ Initial zkey generated`);
    
    console.log(`\n🎲 Contributing randomness to zkey...`);
    const entropy = Array.from({ length: 64 }, () => Math.floor(Math.random() * 16).toString(16)).join('');
    execSync(
      `npx snarkjs zkey contribute "${zkey0Path}" "${zkeyPath}" --name="Rinku Dev Contribution" -e="${entropy}"`,
      { stdio: 'inherit', cwd: ZK_ROOT }
    );
    console.log(`✓ Final zkey generated`);
    
    fs.unlinkSync(zkey0Path);
    
    console.log(`\n📤 Exporting verification key...`);
    execSync(
      `npx snarkjs zkey export verificationkey "${zkeyPath}" "${vkeyPath}"`,
      { stdio: 'inherit', cwd: ZK_ROOT }
    );
    console.log(`✓ Verification key exported`);
    
  } catch (error) {
    console.error('✗ Zkey generation failed');
    throw error;
  }
}

async function verifySetup(): Promise<void> {
  const outputDir = path.join(BUILD_DIR, CIRCUIT_NAME);
  const wasmPath = path.join(outputDir, `${CIRCUIT_NAME}_js`, `${CIRCUIT_NAME}.wasm`);
  const zkeyPath = path.join(outputDir, `${CIRCUIT_NAME}.zkey`);
  const vkeyPath = path.join(outputDir, 'verification_key.json');
  
  console.log(`\n🔍 Verifying build artifacts...`);
  
  const artifacts = [
    { name: 'WASM', path: wasmPath },
    { name: 'Proving Key', path: zkeyPath },
    { name: 'Verification Key', path: vkeyPath },
  ];
  
  let allExist = true;
  for (const artifact of artifacts) {
    if (fs.existsSync(artifact.path)) {
      const stats = fs.statSync(artifact.path);
      const sizeMB = (stats.size / (1024 * 1024)).toFixed(2);
      console.log(`  ✓ ${artifact.name}: ${sizeMB} MB`);
    } else {
      console.log(`  ✗ ${artifact.name}: MISSING`);
      allExist = false;
    }
  }
  
  if (!allExist) {
    throw new Error('Some artifacts are missing');
  }
  
  console.log(`\n✨ Circuit compilation complete!`);
  console.log(`\nArtifacts location: ${outputDir}`);
  console.log(`\nTo use in your application:`);
  console.log(`  WASM: ${wasmPath}`);
  console.log(`  zkey: ${zkeyPath}`);
  console.log(`  vkey: ${vkeyPath}`);
}

async function main(): Promise<void> {
  console.log('═══════════════════════════════════════════════════════════');
  console.log('  Rinku ZK Circuit Compilation');
  console.log('═══════════════════════════════════════════════════════════');
  
  if (!fs.existsSync(BUILD_DIR)) {
    fs.mkdirSync(BUILD_DIR, { recursive: true });
  }
  
  const ptauPath = await downloadPtau();
  await compileCircuit();
  await generateZkey(ptauPath);
  await verifySetup();
}

main().catch((error) => {
  console.error('\n✗ Compilation failed:', error.message);
  process.exit(1);
});
