const getApiBaseUrl = () => {
  const envApiUrl = import.meta.env.VITE_API_URL;

  if (
    envApiUrl &&
    !envApiUrl.includes("127.0.0.1") &&
    !envApiUrl.includes("localhost")
  ) {
    return `${envApiUrl}/api`;
  }

  if (import.meta.env.PROD) {
    const host = window.location.hostname;
    return `https://${host.replace(/-5000\./, "-3001.")}/api`;
  }

  return "/api";
};

export const API_URL = getApiBaseUrl();
