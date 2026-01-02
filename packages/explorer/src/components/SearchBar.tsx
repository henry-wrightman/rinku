import { useState, useCallback, type KeyboardEvent } from "react";

interface SearchResult {
  type: "transaction" | "account" | "contract" | null;
  data: any;
  error?: string;
}

interface SearchBarProps {
  onResult: (result: SearchResult) => void;
}

const NODE_URL = "/api";

export function SearchBar({ onResult }: SearchBarProps) {
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);

  const search = useCallback(async () => {
    if (!query.trim()) return;

    setLoading(true);
    const q = query.trim();

    try {
      const txRes = await fetch(`${NODE_URL}/tx/${q}`);
      if (txRes.ok) {
        const data = await txRes.json();
        onResult({ type: "transaction", data });
        setLoading(false);
        return;
      }

      const accountRes = await fetch(`${NODE_URL}/account/${q}`);
      if (accountRes.ok) {
        const data = await accountRes.json();
        onResult({ type: "account", data });
        setLoading(false);
        return;
      }

      const contractRes = await fetch(`${NODE_URL}/contracts/${q}`);
      if (contractRes.ok) {
        const data = await contractRes.json();
        onResult({ type: "contract", data });
        setLoading(false);
        return;
      }

      onResult({ type: null, data: null, error: "Not found: no matching transaction, account, or contract" });
    } catch (e) {
      onResult({ type: null, data: null, error: "Search failed" });
    } finally {
      setLoading(false);
    }
  }, [query, onResult]);

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      search();
    }
  };

  return (
    <div className="search-bar">
      <input
        type="text"
        placeholder="search tx hash, wallet, or contract..."
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={handleKeyDown}
      />
      <button onClick={search} disabled={loading || !query.trim()}>
        {loading ? "..." : "search"}
      </button>
    </div>
  );
}
