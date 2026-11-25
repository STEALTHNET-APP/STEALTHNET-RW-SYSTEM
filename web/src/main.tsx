import '@fontsource-variable/onest';
import './styles/tokens.css';
import './styles/base.css';
import { lazy, StrictMode, Suspense } from 'react';
import { createRoot } from 'react-dom/client';
import { BrowserRouter, Route, Routes, useLocation } from 'react-router';
import { reloadOnStaleChunks } from './lib/chunks';
import { guessLang, useApplied } from './lib/i18n';
import { captureReferral } from './lib/referral';
import { useSiteData } from './lib/site';
import { servedApp, telegramEntry } from './lib/telegramEntry';
import type { Site } from './lib/types';
import { captureUtm } from './lib/utm';

const AdminApp = lazy(() => import('./admin/AdminApp').then((m) => ({ default: m.AdminApp })));
const PageMissing = lazy(() => import('./site/Soon').then((m) => ({ default: m.PageMissing })));
const PageFailed = lazy(() => import('./site/Soon').then((m) => ({ default: m.PageFailed })));
const CabinetApp = lazy(() => import('./cabinet/CabinetApp').then((m) => ({ default: m.CabinetApp })));
const MiniApp = lazy(() => import('./mini/MiniApp').then((m) => ({ default: m.MiniApp })));
const CabinetPreview = lazy(() => import('./cabinet/preview/CabinetPreview').then((m) => ({ default: m.CabinetPreview })));
const MiniPreview = lazy(() => import('./mini/MiniPreview').then((m) => ({ default: m.MiniPreview })));
const Landing = lazy(() => import('./site/Landing').then((m) => ({ default: m.Landing })));
const LandingPreview = lazy(() => import('./site/Landing').then((m) => ({ default: m.LandingPreview })));
const PayPage = lazy(() => import('./pay/PayPage').then((m) => ({ default: m.PayPage })));
const VerifyPage = lazy(() => import('./verify/VerifyPage').then((m) => ({ default: m.VerifyPage })));
const OpenPage = lazy(() => import('./open/OpenPage').then((m) => ({ default: m.OpenPage })));
const DocumentPage = lazy(() => import('./site/DocumentPage').then((m) => ({ default: m.DocumentPage })));
const DocumentPreview = lazy(() => import('./site/DocumentPage').then((m) => ({ default: m.DocumentPreview })));

function Blank() {
  return <div style={{ minHeight: '100vh', background: 'var(--bg)' }} aria-busy="true" />;
}

function SitePage() {
  const { site, failed, loading, retry } = useSiteData();
  const { pathname } = useLocation();
  const path = pathname.length > 1 ? pathname.replace(/\/+$/, '') : pathname;
  const doc = site?.documents?.find((d) => d.page && d.url === path);
  if (site && doc) {
    return <DocumentPage key={doc.url} site={site} path={doc.url} />;
  }
  return <SiteNote site={site} failed={failed} loading={loading} onRetry={retry} />;
}

function SiteNote({ site, failed, loading, onRetry }: { site: Site | null; failed: boolean; loading: boolean; onRetry: () => void }) {
  const ready = useApplied(site ? guessLang(site) : null);
  if (!site || !ready) {
    return <Blank />;
  }
  return failed ? <PageFailed site={site} busy={loading} onRetry={onRetry} /> : <PageMissing site={site} />;
}

const CABINET_PREVIEW_PATH = '/admin/cabinet-preview';

function App() {
  if (window.location.pathname === CABINET_PREVIEW_PATH) {
    return (
      <Suspense fallback={<Blank />}>
        <CabinetPreview />
      </Suspense>
    );
  }
  return (
    <BrowserRouter>
      <Suspense fallback={<Blank />}>
        <Routes>
          <Route path="/admin/landing-preview" element={<LandingPreview />} />
          <Route path="/admin/document-preview" element={<DocumentPreview />} />
          <Route path="/admin/mini-preview" element={<MiniPreview />} />
          <Route path="/admin/*" element={<AdminApp />} />
          <Route path="/cabinet/*" element={<CabinetApp />} />
          <Route path="/app/*" element={<MiniApp />} />
          <Route path="/pay/:id" element={<PayPage />} />
          <Route path="/verify/:token" element={<VerifyPage />} />
          <Route path="/open-app" element={<OpenPage />} />
          <Route path="/" element={<Landing />} />
          <Route path="*" element={<SitePage />} />
        </Routes>
      </Suspense>
    </BrowserRouter>
  );
}

reloadOnStaleChunks();
captureReferral();
captureUtm();

const root = document.getElementById('root');
const app = telegramEntry(window.location, servedApp(document));
if (app) {
  window.location.replace(app);
} else if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
