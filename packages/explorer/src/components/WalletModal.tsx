import { useState, useEffect, useRef } from "react";
import { serializeKeyPair, type SerializedKeyPair } from "../crypto";
import { useRinku } from "../context/WalletContext";
import { API_URL } from "../config";

interface TransactionItem {
  hash: string;
  from: string;
  to: string;
  amount: number;
  timestamp: number;
  direction: string;
  finalized: boolean;
  memo?: string;
  references?: string[];
  fast_path_status?:
    | "pending"
    | "confirmed"
    | "executed"
    | "finalized"
    | "timeout"
    | "not_eligible";
  fast_path_confirmed_at_ms?: number;
  fast_path_finality_ms?: number;
}

interface WalletModalProps {
  isOpen: boolean;
  onClose: () => void;
}

export function WalletModal({ isOpen, onClose }: WalletModalProps) {
  const {
    wallet: keyPair,
    accountInfo,
    refreshAccount,
    generateNewWallet,
    importWallet,
    logout,
    submitTransaction,
  } = useRinku();

  const [showPrivateKey, setShowPrivateKey] = useState(false);
  const [showSendForm, setShowSendForm] = useState(false);
  const [importKey, setImportKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [sending, setSending] = useState(false);
  const modalRef = useRef<HTMLDivElement>(null);

  const [sendTo, setSendTo] = useState("");
  const [sendAmount, setSendAmount] = useState("");
  const [sendMemo, setSendMemo] = useState("");
  const [sendReferences, setSendReferences] = useState("");

  const [showHistory, setShowHistory] = useState(false);
  const [txHistory, setTxHistory] = useState<TransactionItem[]>([]);
  const [loadingHistory, setLoadingHistory] = useState(false);
  const [expandedTxs, setExpandedTxs] = useState<Set<string>>(new Set());

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        modalRef.current &&
        !modalRef.current.contains(event.target as Node)
      ) {
        onClose();
      }
    };

    if (isOpen) {
      document.addEventListener("mousedown", handleClickOutside);
    }
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [isOpen, onClose]);

  const fetchTransactionHistory = async () => {
    if (!keyPair) return;
    setLoadingHistory(true);
    try {
      const res = await fetch(
        `${API_URL}/account/${keyPair.fingerprint}/transactions`,
      );
      if (res.ok) {
        const data = await res.json();
        setTxHistory(data.transactions || []);
      }
    } catch (e) {
      console.error("Failed to fetch tx history:", e);
    } finally {
      setLoadingHistory(false);
    }
  };

  const handleToggleHistory = () => {
    if (!showHistory) {
      fetchTransactionHistory();
    }
    setShowHistory(!showHistory);
  };

  const formatTime = (ts: number) => {
    const date = new Date(ts);
    return date.toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  };

  const handleGenerate = async () => {
    setError(null);
    setResult(null);
    setLoading(true);
    try {
      await generateNewWallet();
      setShowPrivateKey(true);
      setResult("Wallet created! SAVE YOUR KEY!");
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  };

  const handleImport = () => {
    setError(null);
    setResult(null);
    if (!importKey.trim()) {
      setError("Please paste a key");
      return;
    }

    try {
      importWallet(importKey.trim());
      setImportKey("");
      setResult("Wallet imported!");
    } catch (e: any) {
      setError("Failed to import: " + e.message);
    }
  };

  const handleDisconnect = () => {
    logout();
    setShowPrivateKey(false);
  };

  const copyToClipboard = (text: string) => {
    navigator.clipboard.writeText(text);
    setResult("Copied!");
    setTimeout(() => setResult(null), 2000);
  };

  const toggleTxExpand = (hash: string) => {
    setExpandedTxs((prev) => {
      const next = new Set(prev);
      if (next.has(hash)) {
        next.delete(hash);
      } else {
        next.add(hash);
      }
      return next;
    });
  };

  const formatShortTime = (ts: number) => {
    const d = new Date(ts);
    const now = new Date();
    const diffMs = now.getTime() - d.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    if (diffMins < 1) return "now";
    if (diffMins < 60) return `${diffMins}m`;
    const diffHours = Math.floor(diffMins / 60);
    if (diffHours < 24) return `${diffHours}h`;
    const diffDays = Math.floor(diffHours / 24);
    if (diffDays < 7) return `${diffDays}d`;
    return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  };

  const getTxLabel = (tx: TransactionItem) => {
    if (tx.memo)
      return tx.memo.slice(0, 40) + (tx.memo.length > 40 ? "..." : "");
    if (tx.from === "faucet") return "Faucet";
    return tx.direction === "sent"
      ? `To ${tx.to.slice(0, 6)}...`
      : `From ${tx.from.slice(0, 6)}...`;
  };

  const handleSendTransaction = async () => {
    if (!keyPair || !accountInfo) return;

    setError(null);
    setResult(null);

    if (!sendTo.trim()) {
      setError("Recipient address required");
      return;
    }

    if (sendTo.length !== 40) {
      setError("Invalid address (must be 40 characters)");
      return;
    }

    const amount = sendAmount.trim() ? parseFloat(sendAmount) : 0;
    if (isNaN(amount) || amount < 0) {
      setError("Invalid amount");
      return;
    }

    const gasFee = 0.001;
    if (accountInfo.balance < amount + gasFee) {
      setError(
        `Insufficient balance. Need ${(amount + gasFee).toFixed(4)} RKU`,
      );
      return;
    }

    const references = sendReferences.trim()
      ? sendReferences
          .split(",")
          .map((r) => r.trim())
          .filter((r) => r.length > 0)
          .slice(0, 4)
      : [];

    setSending(true);

    try {
      const txResult = await submitTransaction({
        to: sendTo.trim(),
        amount,
        kind: "transfer",
        memo: sendMemo.trim() || undefined,
        references: references.length > 0 ? references : undefined,
      });

      setResult(`Transaction sent! Hash: ${txResult.hash?.slice(0, 12)}...`);

      setSendTo("");
      setSendAmount("");
      setSendMemo("");
      setSendReferences("");
      setShowSendForm(false);

      setTimeout(refreshAccount, 2000);
    } catch (e: unknown) {
      const errMsg = e instanceof Error ? e.message : "Unknown error";
      setError("Send failed: " + errMsg);
    } finally {
      setSending(false);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="wallet-modal-overlay">
      <div className="wallet-modal" ref={modalRef}>
        <div className="wallet-modal-header">
          <div className="value-with-copy">
            <h3>wallet</h3>
            {keyPair && (
              <button
                className="copy-btn disconnect"
                onClick={handleDisconnect}
              >
                disconnect
              </button>
            )}
          </div>
          <button className="close-btn" onClick={onClose}>
            x
          </button>
        </div>

        {error && <div className="error-message">{error}</div>}
        {result && <div className="success-message">{result}</div>}

        {keyPair ? (
          <div className="wallet-connected">
            <div className="wallet-info-row">
              <span className="label">address</span>
              <div className="value-with-copy">
                <span className="value mono">
                  {keyPair.fingerprint.slice(0, 8)}...
                  {keyPair.fingerprint.slice(-6)}
                </span>
                <button
                  className="copy-btn"
                  onClick={() => copyToClipboard(keyPair.fingerprint)}
                >
                  copy
                </button>
              </div>
            </div>

            <div className="wallet-info-row">
              <span className="label">pubkey</span>
              <div className="value-with-copy">
                <span className="value mono">
                  {keyPair.publicKey.slice(0, 8)}...
                  {keyPair.publicKey.slice(-6)}
                </span>
                <button
                  className="copy-btn"
                  onClick={() => copyToClipboard(keyPair.publicKey)}
                >
                  copy
                </button>
              </div>
            </div>

            <div className="wallet-info-row">
              <span className="label">balance</span>
              <span className="value">
                {accountInfo?.balance?.toFixed(4) || "0.0000"} RKU
              </span>
            </div>

            <div className="wallet-info-row">
              <span className="label">staked</span>
              <span className="value">
                {accountInfo?.staked?.toFixed(4) || "0.0000"} RKU
              </span>
            </div>

            <div className="wallet-info-row">
              <span className="label">nonce</span>
              <span className="value">{accountInfo?.nonce || 0}</span>
            </div>

            <div className="wallet-actions">
              <button
                className="wallet-action-btn send-btn"
                onClick={() => setShowSendForm(!showSendForm)}
              >
                {showSendForm ? "cancel" : "send"}
              </button>
              <button
                className="wallet-action-btn history-btn"
                onClick={handleToggleHistory}
              >
                {showHistory ? "hide history" : "history"}
              </button>
              <button
                className="wallet-action-btn"
                onClick={() => setShowPrivateKey(!showPrivateKey)}
              >
                {showPrivateKey ? "hide key" : "show key"}
              </button>
              <button
                className="wallet-action-btn export"
                onClick={() => copyToClipboard(serializeKeyPair(keyPair))}
              >
                export key
              </button>
            </div>

            {showSendForm && (
              <div className="send-form">
                <div className="send-form-field">
                  <label>To Address *</label>
                  <input
                    type="text"
                    placeholder="40-character address"
                    value={sendTo}
                    onChange={(e) => setSendTo(e.target.value)}
                    maxLength={40}
                  />
                </div>

                <div className="send-form-field">
                  <label>Amount (optional, default: 0)</label>
                  <input
                    type="number"
                    placeholder="0.0"
                    value={sendAmount}
                    onChange={(e) => setSendAmount(e.target.value)}
                    min="0"
                    step="0.0001"
                  />
                  <span className="field-hint">+ 0.001 RKU gas fee</span>
                </div>

                <div className="send-form-field">
                  <label>Memo (optional, max 256 chars)</label>
                  <textarea
                    placeholder="Message content..."
                    value={sendMemo}
                    onChange={(e) => setSendMemo(e.target.value)}
                    maxLength={256}
                    rows={2}
                  />
                  <span className="field-hint">{sendMemo.length}/256</span>
                </div>

                <div className="send-form-field">
                  <label>References (optional, tx hashes)</label>
                  <input
                    type="text"
                    placeholder="hash1, hash2 (comma-separated, max 4)"
                    value={sendReferences}
                    onChange={(e) => setSendReferences(e.target.value)}
                  />
                  <span className="field-hint">
                    Link to previous transactions
                  </span>
                </div>

                <button
                  className="wallet-btn send-submit"
                  onClick={handleSendTransaction}
                  disabled={sending || !sendTo.trim()}
                >
                  {sending ? "sending..." : "send transaction"}
                </button>
              </div>
            )}

            {showPrivateKey && (
              <div className="private-key-display">
                <div className="warning">
                  Keep this secret! Anyone with this key can access your funds.
                </div>
                <textarea
                  readOnly
                  value={serializeKeyPair(keyPair)}
                  className="key-textarea"
                />
              </div>
            )}

            {showHistory && (
              <div className="tx-history">
                <div className="tx-history-header">
                  <span>History ({txHistory.length})</span>
                  <button
                    className="refresh-btn"
                    onClick={fetchTransactionHistory}
                    disabled={loadingHistory}
                  >
                    {loadingHistory ? "..." : "refresh"}
                  </button>
                </div>
                {loadingHistory && (
                  <div className="tx-history-loading">Loading...</div>
                )}
                {!loadingHistory && txHistory.length === 0 && (
                  <div className="tx-history-empty">No transactions yet</div>
                )}
                {!loadingHistory && txHistory.length > 0 && (
                  <div className="tx-history-list compact">
                    {txHistory.map((tx) => {
                      const isExpanded = expandedTxs.has(tx.hash);
                      const hasMessage = !!tx.memo;
                      return (
                        <div
                          key={tx.hash}
                          className={`tx-compact-item ${tx.direction} ${isExpanded ? "expanded" : ""}`}
                          onClick={() => toggleTxExpand(tx.hash)}
                        >
                          <div className="tx-compact-row">
                            <span className={`tx-icon ${tx.direction}`}>
                              {tx.direction === "sent" ? "↑" : "↓"}
                            </span>
                            <span className="tx-compact-label">
                              {getTxLabel(tx)}
                            </span>
                            <span
                              className={`tx-compact-amount ${tx.direction}`}
                            >
                              {tx.direction === "sent" ? "-" : "+"}
                              {tx.amount > 0
                                ? tx.amount.toFixed(tx.amount < 1 ? 4 : 2)
                                : "0"}
                            </span>
                            <span className="tx-compact-time">
                              {formatShortTime(tx.timestamp)}
                            </span>
                            <span
                              className={`tx-compact-status ${tx.fast_path_status === "confirmed" || tx.fast_path_status === "executed" || tx.fast_path_status === "finalized" || tx.finalized ? "ok" : "pending"}`}
                              title={
                                (tx.fast_path_status === "confirmed" ||
                                  tx.fast_path_status === "executed" ||
                                  tx.fast_path_status === "finalized") &&
                                tx.fast_path_finality_ms
                                  ? `Finalized in ${tx.fast_path_finality_ms}ms${tx.finalized ? " · anchored in checkpoint" : ""}`
                                  : tx.finalized
                                    ? "Anchored in checkpoint"
                                    : "Pending finality"
                              }
                            >
                              {tx.fast_path_status === "confirmed" ||
                              tx.fast_path_status === "executed" ||
                              tx.fast_path_status === "finalized" ||
                              tx.finalized
                                ? "✓"
                                : "○"}
                            </span>
                          </div>
                          {isExpanded && (
                            <div className="tx-expanded-details">
                              <div className="tx-detail-row">
                                <span className="detail-label">
                                  {tx.direction === "sent" ? "To:" : "From:"}
                                </span>
                                <span
                                  className="detail-value clickable"
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    copyToClipboard(
                                      tx.direction === "sent" ? tx.to : tx.from,
                                    );
                                  }}
                                >
                                  {tx.direction === "sent" ? tx.to : tx.from}
                                </span>
                              </div>
                              <div className="tx-detail-row">
                                <span className="detail-label">Amount:</span>
                                <span className="detail-value">
                                  {tx.amount.toFixed(4)} RKU
                                </span>
                              </div>
                              <div className="tx-detail-row">
                                <span className="detail-label">Time:</span>
                                <span className="detail-value">
                                  {formatTime(tx.timestamp)}
                                </span>
                              </div>
                              <div className="tx-detail-row">
                                <span className="detail-label">Hash:</span>
                                <span
                                  className="detail-value clickable"
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    copyToClipboard(tx.hash);
                                  }}
                                >
                                  {tx.hash}
                                </span>
                              </div>
                              <div className="tx-detail-row">
                                <span className="detail-label">Status:</span>
                                <span
                                  className={`detail-value ${tx.finalized ? "finalized" : "pending"}`}
                                >
                                  {tx.finalized ? "Anchored" : "Pending"}
                                </span>
                              </div>
                              {tx.memo && (
                                <div className="tx-detail-memo">
                                  <span className="detail-label">Message:</span>
                                  <div className="memo-content">{tx.memo}</div>
                                </div>
                              )}
                              {tx.references && tx.references.length > 0 && (
                                <div className="tx-detail-refs">
                                  <span className="detail-label">
                                    References:
                                  </span>
                                  <div className="refs-list">
                                    {tx.references.map((ref, i) => (
                                      <span
                                        key={i}
                                        className="ref-hash clickable"
                                        onClick={(e) => {
                                          e.stopPropagation();
                                          copyToClipboard(ref);
                                        }}
                                      >
                                        {ref.slice(0, 16)}...
                                      </span>
                                    ))}
                                  </div>
                                </div>
                              )}
                            </div>
                          )}
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            )}
          </div>
        ) : (
          <div className="wallet-disconnected">
            <div className="import-section">
              <textarea
                placeholder="Paste your wallet key JSON here..."
                value={importKey}
                onChange={(e) => setImportKey(e.target.value)}
                className="import-textarea"
              />
              <button className="wallet-btn import" onClick={handleImport}>
                import wallet
              </button>
            </div>

            <div className="divider">or</div>

            <button
              className="wallet-btn generate"
              onClick={handleGenerate}
              disabled={loading}
            >
              {loading ? "generating..." : "generate new wallet"}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
