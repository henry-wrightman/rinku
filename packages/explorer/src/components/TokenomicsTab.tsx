import { useState, useEffect, useRef, useCallback } from "react";
import { useWebSocketContext } from "../context/WebSocketContext";

interface SupplyStats {
  maxSupply: number;
  genesisAllocation: number;
  circulatingSupply: number;
  totalEmitted: number;
  totalBurned: number;
  remainingToEmit: number;
  currentReward: number;
  halvingEpoch: number;
  nextHalvingAt: number;
  halvingInterval: number;
  checkpointHeight: number;
  validatorFeePercent?: number;
  burnPercent?: number;
}

interface EmissionSchedule {
  epoch: number;
  startHeight: number;
  reward: number;
}

interface EmissionInfo {
  currentEpoch: number;
  currentReward: number;
  halvingInterval: number;
  totalHalvings: number;
  minReward: number;
  schedule: EmissionSchedule[];
  stakeWeightPercent: number;
  ageWeightPercent: number;
}

interface SlashEvent {
  id: string;
  validator: string;
  reason: string;
  amount: number;
  percentSlashed: number;
  checkpointHeight: number;
  timestamp: number;
  details?: string;
}

interface SlashingInfo {
  config: {
    doubleSignPercent: number;
    invalidCheckpointPercent: number;
    livenessPercent: number;
    livenessRepeatPercent: number;
    livenessMissThreshold: number;
    unbondingPeriodDays: number;
  };
  events: SlashEvent[];
  totalSlashed: number;
  unbondingQueue: any[];
}

const getApiBaseUrl = () => {
  const envApiUrl = import.meta.env.VITE_API_URL;

  // If VITE_API_URL is set and not localhost, use it directly
  if (
    envApiUrl &&
    !envApiUrl.includes("127.0.0.1") &&
    !envApiUrl.includes("localhost")
  ) {
    console.log("Using VITE_API_URL:", envApiUrl);
    return `${envApiUrl}/api`;
  }

  if (import.meta.env.PROD) {
    // Production on Replit: transform port in hostname
    const host = window.location.hostname;
    console.log(
      "prod api url (Replit)",
      `https://${host.replace(/-5000\./, "-3001.")}/api`,
    );
    return `https://${host.replace(/-5000\./, "-3001.")}/api`;
  }
  return "/api"; // Dev: use Vite proxy
};
const NODE_URL = getApiBaseUrl();

function formatNumber(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(2) + "M";
  if (n >= 1_000) return (n / 1_000).toFixed(2) + "K";
  return n.toFixed(2);
}

function formatDate(ts: number): string {
  return new Date(ts).toLocaleString();
}

export function TokenomicsTab() {
  const [supply, setSupply] = useState<SupplyStats | null>(null);
  const [emission, setEmission] = useState<EmissionInfo | null>(null);
  const [slashing, setSlashing] = useState<SlashingInfo | null>(null);
  const [loading, setLoading] = useState(true);

  const fetchData = useCallback(async () => {
    try {
      const [supplyRes, emissionRes, slashingRes] = await Promise.all([
        fetch(`${NODE_URL}/tokenomics/supply`),
        fetch(`${NODE_URL}/tokenomics/emission`),
        fetch(`${NODE_URL}/tokenomics/slashing`),
      ]);

      if (supplyRes.ok) setSupply(await supplyRes.json());
      if (emissionRes.ok) setEmission(await emissionRes.json());
      if (slashingRes.ok) setSlashing(await slashingRes.json());
    } catch (e) {
      console.error("Failed to fetch tokenomics:", e);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  const { status: wsStatus, lastEvent } = useWebSocketContext();
  const lastTokenRef = useRef(lastEvent);

  useEffect(() => {
    if (!lastEvent || lastEvent === lastTokenRef.current) return;
    lastTokenRef.current = lastEvent;
    if (lastEvent.type === 'CheckpointCreated') {
      fetchData();
    }
  }, [lastEvent]);

  useEffect(() => {
    if (wsStatus === 'connected') return;
    const interval = setInterval(fetchData, 10000);
    return () => clearInterval(interval);
  }, [wsStatus]);

  if (loading) {
    return <div className="loading">loading tokenomics data...</div>;
  }

  const supplyPercent = supply
    ? (supply.circulatingSupply / supply.maxSupply) * 100
    : 0;
  const emittedPercent = supply
    ? (supply.totalEmitted / (supply.maxSupply - supply.genesisAllocation)) *
      100
    : 0;

  return (
    <div className="tokenomics-tab">
      <section className="tokenomics-section">
        <h3>Supply Overview</h3>
        <div className="tokenomics-grid">
          <div className="tokenomics-card">
            <div className="card-label">Max Supply</div>
            <div className="card-value">
              {formatNumber(supply?.maxSupply || 30000000)} RKU
            </div>
          </div>
          <div className="tokenomics-card">
            <div className="card-label">Circulating</div>
            <div className="card-value">
              {formatNumber(supply?.circulatingSupply || 0)} RKU
            </div>
            <div className="card-sub">{supplyPercent.toFixed(2)}% of max</div>
          </div>
          <div className="tokenomics-card">
            <div className="card-label">Total Emitted</div>
            <div className="card-value">
              {formatNumber(supply?.totalEmitted || 0)} RKU
            </div>
            <div className="card-sub">
              {emittedPercent.toFixed(2)}% of emission cap
            </div>
          </div>
          <div className="tokenomics-card">
            <div className="card-label">Total Burned</div>
            <div className="card-value">
              {formatNumber(supply?.totalBurned || 0)} RKU
            </div>
            <div className="card-sub">from gas fees</div>
          </div>
        </div>

        <div className="tokenomics-info" style={{ marginTop: "1rem" }}>
          <p>
            <strong>Adaptive Fee Split:</strong>{" "}
            {supply?.validatorFeePercent?.toFixed(1) || 70}% to validators /{" "}
            {supply?.burnPercent?.toFixed(1) || 30}% burned
          </p>
          <p className="card-sub">
            Validators receive 70%+ of fees until supply reaches 50% of max,
            then burn increases up to 30%
          </p>
        </div>
      </section>

      <section className="tokenomics-section">
        <h3>Emission Schedule</h3>
        <div className="tokenomics-info">
          <p>
            <strong>Current Epoch:</strong> {emission?.currentEpoch || 0} |
            <strong> Current Reward:</strong>{" "}
            {emission?.currentReward?.toFixed(4) || 150} RKU/checkpoint |
            <strong> Weight Split:</strong> {emission?.stakeWeightPercent || 70}
            % stake / {emission?.ageWeightPercent || 30}% age
          </p>
          <p>
            <strong>Checkpoint Height:</strong> {supply?.checkpointHeight || 0}{" "}
            |<strong> Next Halving:</strong>{" "}
            {formatNumber(supply?.nextHalvingAt || 3150000)} (~18 months per
            epoch)
          </p>
        </div>

        <table className="tokenomics-table">
          <thead>
            <tr>
              <th>Epoch</th>
              <th>Start Height</th>
              <th>Reward (RKU)</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody>
            {emission?.schedule.map((s) => (
              <tr
                key={s.epoch}
                className={
                  (emission?.currentEpoch ?? 0) === s.epoch
                    ? "active-epoch"
                    : ""
                }
              >
                <td>{s.epoch}</td>
                <td>{formatNumber(s.startHeight)}</td>
                <td>{s.reward.toFixed(4)}</td>
                <td>
                  {(emission?.currentEpoch ?? 0) === s.epoch ? (
                    <span className="badge active">Active</span>
                  ) : (emission?.currentEpoch ?? 0) > s.epoch ? (
                    <span className="badge completed">Completed</span>
                  ) : (
                    <span className="badge upcoming">Upcoming</span>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section className="tokenomics-section">
        <h3>Slashing Rules</h3>
        <div className="tokenomics-grid">
          <div className="tokenomics-card warning">
            <div className="card-label">Double Sign</div>
            <div className="card-value">
              {slashing?.config.doubleSignPercent || 15}%
            </div>
          </div>
          <div className="tokenomics-card danger">
            <div className="card-label">Invalid Checkpoint</div>
            <div className="card-value">
              {slashing?.config.invalidCheckpointPercent || 25}%
            </div>
          </div>
          <div className="tokenomics-card">
            <div className="card-label">Liveness Failure</div>
            <div className="card-value">
              {slashing?.config.livenessPercent || 5}%
            </div>
            <div className="card-sub">
              after {slashing?.config.livenessMissThreshold || 3} missed
            </div>
          </div>
          <div className="tokenomics-card">
            <div className="card-label">Unbonding Period</div>
            <div className="card-value">
              {slashing?.config.unbondingPeriodDays || 14} days
            </div>
          </div>
        </div>

        <div className="tokenomics-info">
          <p>
            <strong>Total Slashed:</strong>{" "}
            {formatNumber(slashing?.totalSlashed || 0)} RKU
          </p>
        </div>

        {slashing?.events && slashing.events.length > 0 && (
          <>
            <h4>Recent Slash Events</h4>
            <table className="tokenomics-table">
              <thead>
                <tr>
                  <th>Time</th>
                  <th>Validator</th>
                  <th>Reason</th>
                  <th>Amount</th>
                  <th>%</th>
                </tr>
              </thead>
              <tbody>
                {slashing.events
                  .slice(-10)
                  .reverse()
                  .map((e) => (
                    <tr key={e.id}>
                      <td>{formatDate(e.timestamp)}</td>
                      <td className="mono">
                        {e.validator?.slice(0, 12) ?? "unknown"}...
                      </td>
                      <td>{e.reason?.replace("_", " ") ?? "unknown"}</td>
                      <td>{e.amount?.toFixed(4) ?? "0"} RKU</td>
                      <td>{((e.percentSlashed ?? 0) * 100).toFixed(1)}%</td>
                    </tr>
                  ))}
              </tbody>
            </table>
          </>
        )}

        {slashing?.events?.length === 0 && (
          <p className="no-data">No slash events recorded</p>
        )}
      </section>
    </div>
  );
}
