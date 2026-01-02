import React from 'react';
import ReactDOM from 'react-dom/client';
import { BrowserRouter, Routes, Route } from 'react-router-dom';
import App from './App.js';
import TransactionPage from './TransactionPage.js';
import AccountPage from './AccountPage.js';
import './index.css';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<App />} />
        <Route path="/tx/:payload" element={<TransactionPage />} />
        <Route path="/account/:address" element={<AccountPage />} />
      </Routes>
    </BrowserRouter>
  </React.StrictMode>
);
