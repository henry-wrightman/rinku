import { useState, useEffect, useRef } from "react";
import {
  generateKeyPair,
  serializeKeyPair,
  deserializeKeyPair,
  validateSerializedKey,
  type SerializedKeyPair,
} from "../crypto";

const WALLET_STORAGE_KEY = "rinku_wallet";
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
  return "/api";
};
const NODE_URL = getApiBaseUrl();

interface AccountInfo {
  fingerprint: string;
  balance: number;
  nonce: number;
  staked: number;
}

interface WalletModalProps {
  isOpen: boolean;
  onClose: () => void;
  onWalletChange?: (keyPair: SerializedKeyPair | null) => void;
}

export function WalletModal({
  isOpen,
  onClose,
  onWalletChange,
}: WalletModalProps) {
  const [keyPair, setKeyPair] = useState<SerializedKeyPair | null>(null);
  const [accountInfo, setAccountInfo] = useState<AccountInfo | null>(null);
  const [showPrivateKey, setShowPrivateKey] = useState(false);
  const [importKey, setImportKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const modalRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const stored = localStorage.getItem(WALLET_STORAGE_KEY);
    if (stored && validateSerializedKey(stored)) {
      try {
        const kp = deserializeKeyPair(stored);
        setKeyPair(kp);
        onWalletChange?.(kp);
      } catch (e) {
        console.error("Failed to load stored wallet:", e);
      }
    }
  }, []);

  useEffect(() => {
    if (keyPair) {
      fetchAccountInfo();
      const interval = setInterval(fetchAccountInfo, 5000);
      return () => clearInterval(interval);
    }
  }, [keyPair]);

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

  const fetchAccountInfo = async () => {
    if (!keyPair) return;
    try {
      const res = await fetch(`${NODE_URL}/account/${keyPair.fingerprint}`);
      if (res.ok) {
        const data = await res.json();
        setAccountInfo(data);
      }
    } catch (e) {
      console.error("Failed to fetch account:", e);
    }
  };

  const handleGenerate = async () => {
    setError(null);
    setResult(null);
    setLoading(true);
    try {
      const kp = await generateKeyPair();
      const serialized = serializeKeyPair(kp);
      localStorage.setItem(WALLET_STORAGE_KEY, serialized);
      setKeyPair(kp);
      setShowPrivateKey(true);
      setResult("Wallet created! SAVE YOUR KEY!");
      onWalletChange?.(kp);
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

    if (!validateSerializedKey(importKey)) {
      setError("Invalid key format");
      return;
    }

    try {
      const kp = deserializeKeyPair(importKey);
      localStorage.setItem(WALLET_STORAGE_KEY, importKey);
      setKeyPair(kp);
      setImportKey("");
      setResult("Wallet imported!");
      onWalletChange?.(kp);
    } catch (e: any) {
      setError("Failed to import: " + e.message);
    }
  };

  const handleDisconnect = () => {
    localStorage.removeItem(WALLET_STORAGE_KEY);
    setKeyPair(null);
    setAccountInfo(null);
    setShowPrivateKey(false);
    onWalletChange?.(null);
  };

  const copyToClipboard = (text: string) => {
    navigator.clipboard.writeText(text);
    setResult("Copied!");
    setTimeout(() => setResult(null), 2000);
  };

  if (!isOpen) return null;

  return (
    <div className="wallet-modal-overlay">
      <div className="wallet-modal" ref={modalRef}>
        <div className="wallet-modal-header">
          <h3>wallet</h3>
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
              <button
                className="wallet-action-btn disconnect"
                onClick={handleDisconnect}
              >
                disconnect
              </button>
            </div>

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

export function useWallet() {
  const [keyPair, setKeyPair] = useState<SerializedKeyPair | null>(null);

  useEffect(() => {
    const stored = localStorage.getItem(WALLET_STORAGE_KEY);
    if (stored && validateSerializedKey(stored)) {
      try {
        const kp = deserializeKeyPair(stored);
        setKeyPair(kp);
      } catch (e) {
        console.error("Failed to load wallet:", e);
      }
    }

    const handleStorageChange = () => {
      const stored = localStorage.getItem(WALLET_STORAGE_KEY);
      if (stored && validateSerializedKey(stored)) {
        setKeyPair(deserializeKeyPair(stored));
      } else {
        setKeyPair(null);
      }
    };

    window.addEventListener("storage", handleStorageChange);
    return () => window.removeEventListener("storage", handleStorageChange);
  }, []);

  return keyPair;
}
