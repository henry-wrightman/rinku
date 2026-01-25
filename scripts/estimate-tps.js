#!/usr/bin/env tsx
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
var NODE_URL = process.env.NODE_URL || "http://localhost:3001";
var WINDOW_SECS = parseInt(process.env.WINDOW_SECS || "120", 10);
var SAMPLE_MS = parseInt(process.env.SAMPLE_MS || "1000", 10);
var ASSUMED_PROPAGATION_MS = process.env.ASSUMED_PROPAGATION_MS
    ? parseInt(process.env.ASSUMED_PROPAGATION_MS, 10)
    : undefined;
function fetchJson(url) {
    return __awaiter(this, void 0, void 0, function () {
        var res, text;
        return __generator(this, function (_a) {
            switch (_a.label) {
                case 0: return [4 /*yield*/, fetch(url)];
                case 1:
                    res = _a.sent();
                    if (!!res.ok) return [3 /*break*/, 3];
                    return [4 /*yield*/, res.text()];
                case 2:
                    text = _a.sent();
                    throw new Error("HTTP ".concat(res.status, ": ").concat(text));
                case 3: return [2 /*return*/, res.json()];
            }
        });
    });
}
function avg(values) {
    if (values.length === 0)
        return 0;
    return values.reduce(function (a, b) { return a + b; }, 0) / values.length;
}
function fmt(num, digits) {
    if (digits === void 0) { digits = 2; }
    return Number.isFinite(num) ? num.toFixed(digits) : "n/a";
}
function main() {
    return __awaiter(this, void 0, void 0, function () {
        var start, endAt, samples, checkpoints, lastCheckpointHeight, _a, stats, latest, first, last, elapsedSec, ingestTps, avgTips, avgCheckpointHeight, finalizedTps, checkpointRate, sorted, firstCp, lastCp, cpElapsedSec, txFinalized, latencyBoundTps, estimateCandidates, estimatedMaxTps, recent, recentHeights;
        return __generator(this, function (_b) {
            switch (_b.label) {
                case 0:
                    start = Date.now();
                    endAt = start + WINDOW_SECS * 1000;
                    console.log("=".repeat(60));
                    console.log("RINKU TPS ESTIMATOR");
                    console.log("=".repeat(60));
                    console.log("Node: ".concat(NODE_URL));
                    console.log("Window: ".concat(WINDOW_SECS, "s, Sample: ").concat(SAMPLE_MS, "ms"));
                    if (ASSUMED_PROPAGATION_MS !== undefined) {
                        console.log("Assumed propagation: ".concat(ASSUMED_PROPAGATION_MS, "ms"));
                    }
                    console.log("=".repeat(60));
                    samples = [];
                    checkpoints = [];
                    lastCheckpointHeight = -1;
                    _b.label = 1;
                case 1:
                    if (!(Date.now() < endAt)) return [3 /*break*/, 4];
                    return [4 /*yield*/, Promise.all([
                            fetchJson("".concat(NODE_URL, "/api/stats")),
                            fetchJson("".concat(NODE_URL, "/api/checkpoints/latest")),
                        ])];
                case 2:
                    _a = _b.sent(), stats = _a[0], latest = _a[1];
                    samples.push({
                        at: Date.now(),
                        dagNodes: stats.dag_nodes,
                        tips: stats.tips,
                        checkpointHeight: stats.checkpoint_height,
                    });
                    if (latest.height > lastCheckpointHeight) {
                        checkpoints.push(latest);
                        lastCheckpointHeight = latest.height;
                    }
                    return [4 /*yield*/, new Promise(function (r) { return setTimeout(r, SAMPLE_MS); })];
                case 3:
                    _b.sent();
                    return [3 /*break*/, 1];
                case 4:
                    if (samples.length < 2) {
                        console.error("Not enough samples collected to estimate TPS.");
                        process.exit(1);
                    }
                    first = samples[0];
                    last = samples[samples.length - 1];
                    elapsedSec = (last.at - first.at) / 1000;
                    ingestTps = (last.dagNodes - first.dagNodes) / elapsedSec;
                    avgTips = avg(samples.map(function (s) { return s.tips; }));
                    avgCheckpointHeight = avg(samples.map(function (s) { return s.checkpointHeight; }));
                    finalizedTps = null;
                    checkpointRate = null;
                    if (checkpoints.length >= 2) {
                        sorted = checkpoints
                            .slice()
                            .sort(function (a, b) { return a.height - b.height; });
                        firstCp = sorted[0];
                        lastCp = sorted[sorted.length - 1];
                        cpElapsedSec = lastCp.timestamp - firstCp.timestamp;
                        if (cpElapsedSec > 0) {
                            txFinalized = sorted
                                .slice(1)
                                .reduce(function (sum, cp) { return sum + cp.tx_count; }, 0);
                            finalizedTps = txFinalized / cpElapsedSec;
                            checkpointRate = (lastCp.height - firstCp.height) / cpElapsedSec;
                        }
                    }
                    latencyBoundTps = null;
                    if (ASSUMED_PROPAGATION_MS !== undefined && ASSUMED_PROPAGATION_MS > 0) {
                        latencyBoundTps = avgTips / (ASSUMED_PROPAGATION_MS / 1000);
                    }
                    estimateCandidates = [
                        ingestTps,
                        finalizedTps !== null && finalizedTps !== void 0 ? finalizedTps : Infinity,
                        latencyBoundTps !== null && latencyBoundTps !== void 0 ? latencyBoundTps : Infinity,
                    ].filter(function (v) { return Number.isFinite(v); });
                    estimatedMaxTps = Math.min.apply(Math, estimateCandidates);
                    console.log("\nRESULTS");
                    console.log("-".repeat(60));
                    console.log("Observed ingest TPS (DAG growth): ".concat(fmt(ingestTps)));
                    console.log("Avg tip count: ".concat(fmt(avgTips, 1)));
                    console.log("Avg checkpoint height: ".concat(fmt(avgCheckpointHeight, 2)));
                    if (finalizedTps !== null) {
                        console.log("Finalized TPS (checkpoint tx_count): ".concat(fmt(finalizedTps)));
                    }
                    else {
                        console.log("Finalized TPS (checkpoint tx_count): n/a (not enough checkpoints)");
                    }
                    if (checkpointRate !== null) {
                        console.log("Checkpoint rate: ".concat(fmt(checkpointRate, 3), " / sec"));
                    }
                    if (latencyBoundTps !== null) {
                        console.log("Latency-bound TPS (tips / propagation): ".concat(fmt(latencyBoundTps)));
                    }
                    console.log("-".repeat(60));
                    console.log("Estimated max TPS (min of signals): ".concat(fmt(estimatedMaxTps)));
                    console.log("\nASSUMPTIONS");
                    console.log("-".repeat(60));
                    console.log("- DAG ingest TPS uses dag_nodes delta; assumes each node ~= 1 tx.");
                    console.log("- Finalized TPS uses checkpoint tx_count; tx_count is checkpoint tip_count.");
                    console.log("- Latency-bound TPS uses avg tips / assumed propagation if provided.");
                    console.log("-".repeat(60));
                    return [4 /*yield*/, fetchJson("".concat(NODE_URL, "/api/checkpoints"))];
                case 5:
                    recent = _b.sent();
                    recentHeights = recent.checkpoints
                        .slice(0, 5)
                        .map(function (c) { return c.height; })
                        .join(", ");
                    console.log("Recent checkpoints (latest 5): ".concat(recentHeights));
                    console.log("=".repeat(60));
                    return [2 /*return*/];
            }
        });
    });
}
main().catch(function (err) {
    console.error("TPS estimator failed:", err);
    process.exit(1);
});
