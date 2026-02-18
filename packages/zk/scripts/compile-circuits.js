"use strict";
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    function adopt(value) { return value instanceof P ? value : new P(function (resolve) { resolve(value); }); }
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : adopt(result.value).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
var __generator = (this && this.__generator) || function (thisArg, body) {
    var _ = { label: 0, sent: function() { if (t[0] & 1) throw t[1]; return t[1]; }, trys: [], ops: [] }, f, y, t, g = Object.create((typeof Iterator === "function" ? Iterator : Object).prototype);
    return g.next = verb(0), g["throw"] = verb(1), g["return"] = verb(2), typeof Symbol === "function" && (g[Symbol.iterator] = function() { return this; }), g;
    function verb(n) { return function (v) { return step([n, v]); }; }
    function step(op) {
        if (f) throw new TypeError("Generator is already executing.");
        while (g && (g = 0, op[0] && (_ = 0)), _) try {
            if (f = 1, y && (t = op[0] & 2 ? y["return"] : op[0] ? y["throw"] || ((t = y["return"]) && t.call(y), 0) : y.next) && !(t = t.call(y, op[1])).done) return t;
            if (y = 0, t) op = [op[0] & 2, t.value];
            switch (op[0]) {
                case 0: case 1: t = op; break;
                case 4: _.label++; return { value: op[1], done: false };
                case 5: _.label++; y = op[1]; op = [0]; continue;
                case 7: op = _.ops.pop(); _.trys.pop(); continue;
                default:
                    if (!(t = _.trys, t = t.length > 0 && t[t.length - 1]) && (op[0] === 6 || op[0] === 2)) { _ = 0; continue; }
                    if (op[0] === 3 && (!t || (op[1] > t[0] && op[1] < t[3]))) { _.label = op[1]; break; }
                    if (op[0] === 6 && _.label < t[1]) { _.label = t[1]; t = op; break; }
                    if (t && _.label < t[2]) { _.label = t[2]; _.ops.push(op); break; }
                    if (t[2]) _.ops.pop();
                    _.trys.pop(); continue;
            }
            op = body.call(thisArg, _);
        } catch (e) { op = [6, e]; y = 0; } finally { f = t = 0; }
        if (op[0] & 5) throw op[1]; return { value: op[0] ? op[1] : void 0, done: true };
    }
};
Object.defineProperty(exports, "__esModule", { value: true });
var child_process_1 = require("child_process");
var fs = require("fs");
var path = require("path");
var crypto = require("crypto");
var url_1 = require("url");
var __filename = (0, url_1.fileURLToPath)(import.meta.url);
var __dirname = path.dirname(__filename);
var ZK_ROOT = path.resolve(__dirname, '..');
var WORKSPACE_ROOT = path.resolve(ZK_ROOT, '../..');
var BUILD_DIR = path.join(ZK_ROOT, 'build');
var CIRCUITS_DIR = path.join(ZK_ROOT, 'circuits');
var CIRCUIT_NAME = 'rinku_private_proof';
var PTAU_URL = 'https://storage.googleapis.com/zkevm/ptau/powersOfTau28_hez_final_14.ptau';
var PTAU_FILE = 'powersOfTau28_hez_final_14.ptau';
function downloadPtau() {
    return __awaiter(this, void 0, void 0, function () {
        var ptauPath;
        return __generator(this, function (_a) {
            ptauPath = path.join(BUILD_DIR, PTAU_FILE);
            if (fs.existsSync(ptauPath)) {
                console.log("\u2713 Powers of Tau file already exists: ".concat(PTAU_FILE));
                return [2 /*return*/, ptauPath];
            }
            console.log("\u2B07 Downloading Powers of Tau ceremony file...");
            console.log("  Source: ".concat(PTAU_URL));
            try {
                (0, child_process_1.execSync)("curl -L -o \"".concat(ptauPath, "\" \"").concat(PTAU_URL, "\""), { stdio: 'inherit' });
                console.log("\u2713 Downloaded: ".concat(PTAU_FILE));
                return [2 /*return*/, ptauPath];
            }
            catch (error) {
                console.error('✗ Failed to download Powers of Tau file');
                throw error;
            }
            return [2 /*return*/];
        });
    });
}
function compileCircuit() {
    return __awaiter(this, void 0, void 0, function () {
        var circuitPath, outputDir, nodeModulesPath;
        return __generator(this, function (_a) {
            circuitPath = path.join(CIRCUITS_DIR, "".concat(CIRCUIT_NAME, ".circom"));
            outputDir = path.join(BUILD_DIR, CIRCUIT_NAME);
            if (!fs.existsSync(outputDir)) {
                fs.mkdirSync(outputDir, { recursive: true });
            }
            console.log("\n\u2699 Compiling circuit: ".concat(CIRCUIT_NAME, ".circom"));
            try {
                nodeModulesPath = path.join(WORKSPACE_ROOT, 'node_modules');
                (0, child_process_1.execSync)("circom \"".concat(circuitPath, "\" --r1cs --wasm --sym -o \"").concat(outputDir, "\" -l \"").concat(nodeModulesPath, "\""), { stdio: 'inherit', cwd: ZK_ROOT });
                console.log("\u2713 Circuit compiled successfully");
            }
            catch (error) {
                console.error('✗ Circuit compilation failed');
                throw error;
            }
            return [2 /*return*/];
        });
    });
}
function generateZkey(ptauPath) {
    return __awaiter(this, void 0, void 0, function () {
        var outputDir, r1csPath, zkey0Path, zkeyPath, vkeyPath, entropy;
        return __generator(this, function (_a) {
            outputDir = path.join(BUILD_DIR, CIRCUIT_NAME);
            r1csPath = path.join(outputDir, "".concat(CIRCUIT_NAME, ".r1cs"));
            zkey0Path = path.join(outputDir, "".concat(CIRCUIT_NAME, "_0.zkey"));
            zkeyPath = path.join(outputDir, "".concat(CIRCUIT_NAME, ".zkey"));
            vkeyPath = path.join(outputDir, 'verification_key.json');
            console.log("\n\uD83D\uDD10 Generating proving key (zkey)...");
            try {
                (0, child_process_1.execSync)("npx snarkjs groth16 setup \"".concat(r1csPath, "\" \"").concat(ptauPath, "\" \"").concat(zkey0Path, "\""), { stdio: 'inherit', cwd: ZK_ROOT });
                console.log("\u2713 Initial zkey generated");
                console.log("\n\uD83C\uDFB2 Contributing randomness to zkey...");
                entropy = crypto.randomBytes(32).toString('hex');
                (0, child_process_1.execSync)("npx snarkjs zkey contribute \"".concat(zkey0Path, "\" \"").concat(zkeyPath, "\" --name=\"Rinku Dev Contribution\" -e=\"").concat(entropy, "\""), { stdio: 'inherit', cwd: ZK_ROOT });
                console.log("\u2713 Final zkey generated");
                fs.unlinkSync(zkey0Path);
                console.log("\n\uD83D\uDCE4 Exporting verification key...");
                (0, child_process_1.execSync)("npx snarkjs zkey export verificationkey \"".concat(zkeyPath, "\" \"").concat(vkeyPath, "\""), { stdio: 'inherit', cwd: ZK_ROOT });
                console.log("\u2713 Verification key exported");
            }
            catch (error) {
                console.error('✗ Zkey generation failed');
                throw error;
            }
            return [2 /*return*/];
        });
    });
}
function verifySetup() {
    return __awaiter(this, void 0, void 0, function () {
        var outputDir, wasmPath, zkeyPath, vkeyPath, artifacts, allExist, _i, artifacts_1, artifact, stats, sizeMB;
        return __generator(this, function (_a) {
            outputDir = path.join(BUILD_DIR, CIRCUIT_NAME);
            wasmPath = path.join(outputDir, "".concat(CIRCUIT_NAME, "_js"), "".concat(CIRCUIT_NAME, ".wasm"));
            zkeyPath = path.join(outputDir, "".concat(CIRCUIT_NAME, ".zkey"));
            vkeyPath = path.join(outputDir, 'verification_key.json');
            console.log("\n\uD83D\uDD0D Verifying build artifacts...");
            artifacts = [
                { name: 'WASM', path: wasmPath },
                { name: 'Proving Key', path: zkeyPath },
                { name: 'Verification Key', path: vkeyPath },
            ];
            allExist = true;
            for (_i = 0, artifacts_1 = artifacts; _i < artifacts_1.length; _i++) {
                artifact = artifacts_1[_i];
                if (fs.existsSync(artifact.path)) {
                    stats = fs.statSync(artifact.path);
                    sizeMB = (stats.size / (1024 * 1024)).toFixed(2);
                    console.log("  \u2713 ".concat(artifact.name, ": ").concat(sizeMB, " MB"));
                }
                else {
                    console.log("  \u2717 ".concat(artifact.name, ": MISSING"));
                    allExist = false;
                }
            }
            if (!allExist) {
                throw new Error('Some artifacts are missing');
            }
            console.log("\n\u2728 Circuit compilation complete!");
            console.log("\nArtifacts location: ".concat(outputDir));
            console.log("\nTo use in your application:");
            console.log("  WASM: ".concat(wasmPath));
            console.log("  zkey: ".concat(zkeyPath));
            console.log("  vkey: ".concat(vkeyPath));
            return [2 /*return*/];
        });
    });
}
function main() {
    return __awaiter(this, void 0, void 0, function () {
        var ptauPath;
        return __generator(this, function (_a) {
            switch (_a.label) {
                case 0:
                    console.log('═══════════════════════════════════════════════════════════');
                    console.log('  Rinku ZK Circuit Compilation');
                    console.log('═══════════════════════════════════════════════════════════');
                    if (!fs.existsSync(BUILD_DIR)) {
                        fs.mkdirSync(BUILD_DIR, { recursive: true });
                    }
                    return [4 /*yield*/, downloadPtau()];
                case 1:
                    ptauPath = _a.sent();
                    return [4 /*yield*/, compileCircuit()];
                case 2:
                    _a.sent();
                    return [4 /*yield*/, generateZkey(ptauPath)];
                case 3:
                    _a.sent();
                    return [4 /*yield*/, verifySetup()];
                case 4:
                    _a.sent();
                    return [2 /*return*/];
            }
        });
    });
}
main().catch(function (error) {
    console.error('\n✗ Compilation failed:', error.message);
    process.exit(1);
});
