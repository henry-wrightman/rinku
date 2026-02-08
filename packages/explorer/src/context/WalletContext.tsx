import {
  createContext,
  useContext,
  useState,
  useEffect,
  useRef,
  useCallback,
  type ReactNode,
} from "react";
import {
  generateKeyPair,
  serializeKeyPair,
  deserializeKeyPair,
  validateSerializedKey,
  createSignedTransaction,
  type SerializedKeyPair,
} from "../crypto";
import { API_URL } from "../config";

const WALLET_STORAGE_KEY = "rinku_wallet";

export interface AccountInfo {
  fingerprint: string;
  balance: number;
  nonce: number;
  staked: number;
}

export interface SubmitTxPayload {
  to: string;
  amount: number;
  kind?: string;
  gasPrice?: number;
  memo?: string;
  references?: string[];
  parentCount?: number;
}

export interface SubmitTxResult {
  hash: string;
  [key: string]: unknown;
}

interface WalletContextType {
  wallet: SerializedKeyPair | null;
  accountInfo: AccountInfo | null;
  refreshAccount: () => Promise<AccountInfo | null>;
  generateNewWallet: () => Promise<SerializedKeyPair>;
  importWallet: (key: string) => SerializedKeyPair;
  logout: () => void;
  submitTransaction: (payload: SubmitTxPayload) => Promise<SubmitTxResult>;
  fetchTips: () => Promise<string[]>;
}

const WalletContext = createContext<WalletContextType | null>(null);

export function useRinku(): WalletContextType {
  const ctx = useContext(WalletContext);
  if (!ctx) {
    throw new Error("useRinku must be used within a WalletProvider");
  }
  return ctx;
}

export function WalletProvider({ children }: { children: ReactNode }) {
  const [wallet, setWallet] = useState<SerializedKeyPair | null>(null);
  const [accountInfo, setAccountInfo] = useState<AccountInfo | null>(null);

  const localNonceRef = useRef<number | null>(null);
  const txQueueRef = useRef<Promise<unknown>>(Promise.resolve());

  useEffect(() => {
    const stored = localStorage.getItem(WALLET_STORAGE_KEY);
    if (stored && validateSerializedKey(stored)) {
      try {
        const kp = deserializeKeyPair(stored);
        setWallet(kp);
      } catch (e) {
        console.error("Failed to load stored wallet:", e);
      }
    }

    const handleStorageChange = () => {
      const stored = localStorage.getItem(WALLET_STORAGE_KEY);
      if (stored && validateSerializedKey(stored)) {
        try {
          setWallet(deserializeKeyPair(stored));
        } catch {
          setWallet(null);
        }
      } else {
        setWallet(null);
      }
    };

    window.addEventListener("storage", handleStorageChange);
    window.addEventListener("rinku_wallet_changed", handleStorageChange);
    return () => {
      window.removeEventListener("storage", handleStorageChange);
      window.removeEventListener("rinku_wallet_changed", handleStorageChange);
    };
  }, []);

  const refreshAccount = useCallback(async (): Promise<AccountInfo | null> => {
    if (!wallet) return null;
    try {
      const res = await fetch(`${API_URL}/account/${wallet.fingerprint}`);
      if (res.ok) {
        const data = await res.json();
        const info: AccountInfo = {
          fingerprint: wallet.fingerprint,
          balance: data.balance ?? 0,
          nonce: data.nonce ?? 0,
          staked: data.staked ?? 0,
        };
        setAccountInfo(info);
        localNonceRef.current = info.nonce;
        return info;
      } else {
        const info: AccountInfo = {
          fingerprint: wallet.fingerprint,
          balance: 0,
          nonce: 0,
          staked: 0,
        };
        setAccountInfo(info);
        localNonceRef.current = 0;
        return info;
      }
    } catch (e) {
      console.error("Failed to fetch account:", e);
      return accountInfo;
    }
  }, [wallet]);

  useEffect(() => {
    if (wallet) {
      refreshAccount();
      const interval = setInterval(refreshAccount, 5000);
      return () => clearInterval(interval);
    } else {
      setAccountInfo(null);
      localNonceRef.current = null;
    }
  }, [wallet, refreshAccount]);

  const fetchTips = useCallback(async (): Promise<string[]> => {
    try {
      const res = await fetch(`${API_URL}/tips`);
      if (res.ok) {
        const data = await res.json();
        return data.tips || data || [];
      }
    } catch (e) {
      console.error("Failed to fetch tips:", e);
    }
    return [];
  }, []);

  const submitTransactionInner = async (
    payload: SubmitTxPayload,
  ): Promise<SubmitTxResult> => {
    if (!wallet) throw new Error("Wallet not connected");

    if (localNonceRef.current === null) {
      await refreshAccount();
    }
    const nonce = localNonceRef.current ?? 0;

    const tips = await fetchTips();
    const parentCount = payload.parentCount ?? 8;

    const signedTx = await createSignedTransaction(wallet, {
      to: payload.to,
      amount: payload.amount,
      nonce,
      parents: tips.slice(0, parentCount),
      kind: payload.kind || "transfer",
      gasPrice: payload.gasPrice ?? 0.001,
      memo: payload.memo,
      references: payload.references,
    });

    const res = await fetch(`${API_URL}/tx`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(signedTx),
    });

    if (!res.ok) {
      const errData = await res
        .json()
        .catch(() => ({ error: "Failed to submit" }));
      const errMsg = errData.error || "Transaction failed";

      if (
        errMsg.toLowerCase().includes("nonce") ||
        errMsg.toLowerCase().includes("invalid nonce")
      ) {
        const expectedMatch = errMsg.match(/expected\s+(\d+)/i);
        let retryNonce: number;

        if (expectedMatch) {
          retryNonce = parseInt(expectedMatch[1], 10);
          localNonceRef.current = retryNonce;
          setAccountInfo((prev) =>
            prev ? { ...prev, nonce: retryNonce } : prev,
          );
        } else {
          const freshAccount = await refreshAccount();
          if (!freshAccount) throw new Error(errMsg);
          retryNonce = freshAccount.nonce;
        }

        const retryTips = await fetchTips();
        const retrySigned = await createSignedTransaction(wallet, {
          to: payload.to,
          amount: payload.amount,
          nonce: retryNonce,
          parents: retryTips.slice(0, parentCount),
          kind: payload.kind || "transfer",
          gasPrice: payload.gasPrice ?? 0.001,
          memo: payload.memo,
          references: payload.references,
        });

        const retryRes = await fetch(`${API_URL}/tx`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(retrySigned),
        });

        if (!retryRes.ok) {
          const retryErr = await retryRes
            .json()
            .catch(() => ({ error: "Retry failed" }));
          throw new Error(retryErr.error || "Transaction retry failed");
        }

        const retryResult = await retryRes.json();
        localNonceRef.current = retryNonce + 1;
        setAccountInfo((prev) =>
          prev ? { ...prev, nonce: retryNonce + 1 } : prev,
        );
        return retryResult;
      }

      throw new Error(errMsg);
    }

    const result = await res.json();
    localNonceRef.current = nonce + 1;
    setAccountInfo((prev) => (prev ? { ...prev, nonce: nonce + 1 } : prev));
    return result;
  };

  const submitTransaction = useCallback(
    (payload: SubmitTxPayload): Promise<SubmitTxResult> => {
      const promise = txQueueRef.current.then(() =>
        submitTransactionInner(payload),
      );
      txQueueRef.current = promise.catch(() => {});
      return promise;
    },
    [wallet, refreshAccount, fetchTips],
  );

  const generateNewWallet = useCallback(async (): Promise<SerializedKeyPair> => {
    const kp = await generateKeyPair();
    const serialized = serializeKeyPair(kp);
    localStorage.setItem(WALLET_STORAGE_KEY, serialized);
    setWallet(kp);
    localNonceRef.current = null;
    window.dispatchEvent(new Event("rinku_wallet_changed"));
    return kp;
  }, []);

  const importWallet = useCallback((key: string): SerializedKeyPair => {
    if (!validateSerializedKey(key)) {
      throw new Error("Invalid key format");
    }
    const kp = deserializeKeyPair(key);
    localStorage.setItem(WALLET_STORAGE_KEY, serializeKeyPair(kp));
    setWallet(kp);
    localNonceRef.current = null;
    window.dispatchEvent(new Event("rinku_wallet_changed"));
    return kp;
  }, []);

  const logout = useCallback(() => {
    localStorage.removeItem(WALLET_STORAGE_KEY);
    setWallet(null);
    setAccountInfo(null);
    localNonceRef.current = null;
    window.dispatchEvent(new Event("rinku_wallet_changed"));
  }, []);

  return (
    <WalletContext.Provider
      value={{
        wallet,
        accountInfo,
        refreshAccount,
        generateNewWallet,
        importWallet,
        logout,
        submitTransaction,
        fetchTips,
      }}
    >
      {children}
    </WalletContext.Provider>
  );
}
