import React from 'react';
import ReactDOM from 'react-dom/client';
import { BrowserRouter, Routes, Route } from 'react-router-dom';
import App from './App.js';
import TransactionPage from './TransactionPage.js';
import HashTransactionPage from './HashTransactionPage.js';
import AccountPage from './AccountPage.js';
import { WalletProvider } from './context/WalletContext';
import './index.css';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <WalletProvider>
      <BrowserRouter>
        <Routes>
          <Route path="/" element={<App />} />
          <Route path="/tx/h/:hash" element={<HashTransactionPage />} />
          <Route path="/tx/:payload" element={<TransactionPage />} />
          <Route path="/account/:address" element={<AccountPage />} />
        </Routes>
      </BrowserRouter>
    </WalletProvider>
  </React.StrictMode>
);
