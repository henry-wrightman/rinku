import { useState, useEffect } from 'react';

const THEME_KEY = 'rinku-theme';

export function useTheme() {
  const [darkMode, setDarkMode] = useState(() => {
    const stored = localStorage.getItem(THEME_KEY);
    return stored === null ? true : stored === 'dark';
  });

  useEffect(() => {
    document.body.classList.toggle('light', !darkMode);
    localStorage.setItem(THEME_KEY, darkMode ? 'dark' : 'light');
  }, [darkMode]);

  const toggleTheme = () => setDarkMode(prev => !prev);

  return { darkMode, setDarkMode, toggleTheme };
}
