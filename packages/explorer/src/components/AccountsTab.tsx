import { useState } from "react";
import { Link } from "react-router-dom";
import type { Account } from "../types";
import { truncate } from "../utils";
import { Pagination } from "./Pagination";

interface AccountsTabProps {
  accounts: Account[];
}

const PAGE_SIZE = 20;

export function AccountsTab({ accounts }: AccountsTabProps) {
  const [page, setPage] = useState(0);

  if (accounts.length === 0) {
    return <div className="empty">no accounts yet</div>;
  }

  const sortedAccounts = [...accounts].sort((a, b) => b.balance - a.balance);
  const pageAccounts = sortedAccounts.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE);

  return (
    <div className="section">
      <table>
        <thead>
          <tr>
            <th>address</th>
            <th>balance</th>
            <th>nonce</th>
          </tr>
        </thead>
        <tbody>
          {pageAccounts.map((account) => (
            <tr key={account.fingerprint}>
              <td className="hash">
                <Link 
                  to={`/account/${account.fingerprint}`}
                  style={{ color: "#b48ead", textDecoration: "underline" }}
                >
                  {truncate(account.fingerprint, 16)}
                </Link>
              </td>
              <td className="amount">{account.balance.toLocaleString()}</td>
              <td>{account.nonce}</td>
            </tr>
          ))}
        </tbody>
      </table>

      <Pagination
        page={page}
        totalItems={accounts.length}
        pageSize={PAGE_SIZE}
        onPageChange={setPage}
      />
    </div>
  );
}
