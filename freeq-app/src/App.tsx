import { useState, useCallback, useEffect, useRef } from 'react';
import { useStore } from './store';
import { useKeyboard } from './hooks/useKeyboard';
import { setUnreadCount } from './lib/notifications';
import { ConnectScreen } from './components/ConnectScreen';
import { Sidebar } from './components/Sidebar';
import { TopBar } from './components/TopBar';
import { MessageList } from './components/MessageList';
import { ComposeBox } from './components/ComposeBox';
import { MemberList } from './components/MemberList';
import { QuickSwitcher } from './components/QuickSwitcher';
import { SettingsPanel } from './components/SettingsPanel';
import { ReconnectBanner } from './components/ReconnectBanner';
import { GuestUpgradeBanner } from './components/GuestUpgradeBanner';
import { ImageLightbox } from './components/ImageLightbox';
import { SearchModal } from './components/SearchModal';
import { ChannelListModal } from './components/ChannelListModal';
import { ThreadView } from './components/ThreadView';
import { JoinGateModal } from './components/JoinGateModal';
import { ChannelSettingsPanel } from './components/ChannelSettingsPanel';
import { KeyboardShortcuts } from './components/KeyboardShortcuts';
import { ToastContainer } from './components/Toast';
import { FileDropOverlay } from './components/FileDropOverlay';
import { InstallPrompt } from './components/InstallPrompt';
import { OnboardingTour } from './components/OnboardingTour';
import { BookmarksPanel } from './components/BookmarksPanel';
import { MotdBanner } from './components/MotdBanner';

export default function App() {
  const registered = useStore((s) => s.registered);
  const theme = useStore((s) => s.theme);
  const [quickSwitcher, setQuickSwitcher] = useState(false);
  const [settings, setSettings] = useState(false);
  const [shortcuts, setShortcuts] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [membersOpen, setMembersOpen] = useState(() => window.innerWidth >= 768);
  const threadMsgId = useStore((s) => s.threadMsgId);
  const threadChannel = useStore((s) => s.threadChannel);
  const channels = useStore((s) => s.channels);
  const activeChannel = useStore((s) => s.activeChannel);
  const setActive = useStore((s) => s.setActiveChannel);

  // Swipe gesture to open/close sidebar on mobile
  const touchStart = useRef<{ x: number; y: number } | null>(null);
  useEffect(() => {
    const onStart = (e: TouchEvent) => {
      touchStart.current = { x: e.touches[0].clientX, y: e.touches[0].clientY };
    };
    const onEnd = (e: TouchEvent) => {
      if (!touchStart.current) return;
      const dx = e.changedTouches[0].clientX - touchStart.current.x;
      const dy = Math.abs(e.changedTouches[0].clientY - touchStart.current.y);
      // Horizontal swipe (>80px, mostly horizontal)
      if (Math.abs(dx) > 80 && dy < 60 && window.innerWidth < 768) {
        if (dx > 0 && touchStart.current.x < 40) setSidebarOpen(true);
        else if (dx < 0 && sidebarOpen) setSidebarOpen(false);
      }
      touchStart.current = null;
    };
    document.addEventListener('touchstart', onStart, { passive: true });
    document.addEventListener('touchend', onEnd, { passive: true });
    return () => { document.removeEventListener('touchstart', onStart); document.removeEventListener('touchend', onEnd); };
  }, [sidebarOpen]);

  // Apply theme to document
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
  }, [theme]);

  // Mobile keyboard: resize layout when virtual keyboard appears
  useEffect(() => {
    const vv = window.visualViewport;
    if (!vv) return;
    const onResize = () => {
      // When keyboard is open, visualViewport.height < window.innerHeight
      const offset = window.innerHeight - vv.height;
      document.documentElement.style.setProperty('--vk-offset', `${offset}px`);
    };
    vv.addEventListener('resize', onResize);
    vv.addEventListener('scroll', onResize);
    return () => { vv.removeEventListener('resize', onResize); vv.removeEventListener('scroll', onResize); };
  }, []);

  // Request notification permission when registered
  useEffect(() => {
    // Notification permission is deferred to first mention (see notifications.ts)
  }, [registered]);

  // Handle invite link auto-join when already connected
  useEffect(() => {
    if (!registered) return;
    const hash = window.location.hash;
    if (hash.startsWith('#auto-join=')) {
      const ch = decodeURIComponent(hash.slice('#auto-join='.length));
      window.history.replaceState(null, '', window.location.pathname);
      // Join and switch to the channel
      import('./irc/client').then(({ joinChannel }) => {
        joinChannel(ch);
        setActive(ch);
      });
    }
  }, [registered, setActive]);

  // Close sidebar and member list on mobile when switching channels
  useEffect(() => {
    if (window.innerWidth < 768) {
      setSidebarOpen(false);
      setMembersOpen(false);
    }
  }, [activeChannel]);

  // Ensure modals close when we disconnect
  useEffect(() => {
    if (!registered) {
      setQuickSwitcher(false);
      setSettings(false);
      setShortcuts(false);
      useStore.getState().setSearchOpen(false);
      useStore.getState().setChannelListOpen(false);
      useStore.getState().setLightboxUrl(null);
      useStore.getState().closeThread();
    }
  }, [registered]);

  // Total unread for title badge
  const totalUnread = [...channels.values()].reduce((sum, ch) => sum + ch.unreadCount, 0);
  setUnreadCount(totalUnread);

  // Global keyboard shortcuts
  const switchToNth = useCallback((n: number) => {
    const favorites = useStore.getState().favorites;
    const allJoined = [...channels.values()].filter((ch) => ch.isJoined);
    const allChans = allJoined
      .filter((ch) => ch.name.startsWith('#') || ch.name.startsWith('&'))
      .sort((a, b) => a.name.localeCompare(b.name));
    const favList = allChans.filter((ch) => favorites.has(ch.name.toLowerCase()));
    const chanList = allChans.filter((ch) => !favorites.has(ch.name.toLowerCase()));
    const dmList = allJoined
      .filter((ch) => !ch.name.startsWith('#') && !ch.name.startsWith('&') && ch.name !== 'server')
      .sort((a, b) => a.name.localeCompare(b.name));
    const ordered = [...favList, ...chanList, ...dmList];
    if (ordered[n]) setActive(ordered[n].name);
  }, [channels, setActive]);

  const switchChannel = useCallback((dir: -1 | 1) => {
    const sorted = [...channels.values()]
      .filter((ch) => ch.isJoined)
      .sort((a, b) => a.name.localeCompare(b.name));
    const idx = sorted.findIndex((ch) => ch.name.toLowerCase() === activeChannel.toLowerCase());
    const next = sorted[idx + dir];
    if (next) setActive(next.name);
  }, [channels, activeChannel, setActive]);

  useKeyboard(registered ? {
    'mod+k': () => setQuickSwitcher(true),
    'mod+f': () => useStore.getState().setSearchOpen(true),
    'alt+ArrowUp': () => switchChannel(-1),
    'alt+ArrowDown': () => switchChannel(1),
    'mod+/': () => setShortcuts(true),
    'mod+b': () => useStore.getState().setBookmarksPanelOpen(true),
    'alt+1': () => switchToNth(0),
    'alt+2': () => switchToNth(1),
    'alt+3': () => switchToNth(2),
    'alt+4': () => switchToNth(3),
    'alt+5': () => switchToNth(4),
    'alt+6': () => switchToNth(5),
    'alt+7': () => switchToNth(6),
    'alt+8': () => switchToNth(7),
    'alt+9': () => switchToNth(8),
    'alt+0': () => switchToNth(9),
    'escape': () => {
      setQuickSwitcher(false);
      setSettings(false);
      setShortcuts(false);
      useStore.getState().setSearchOpen(false);
      useStore.getState().setChannelListOpen(false);
      useStore.getState().setLightboxUrl(null);
      useStore.getState().closeThread();
    },
  } : {}, [channels, switchToNth, registered]);

  if (!registered) {
    return (
      <div className="h-dvh flex flex-col bg-bg">
        <ConnectScreen />
      </div>
    );
  }

  return (
    <div className="flex flex-col bg-bg" style={{ height: 'calc(100dvh - var(--vk-offset, 0px))' }}>
      <ReconnectBanner />
      <GuestUpgradeBanner />
      <div className="flex flex-1 min-h-0">
        {/* Mobile sidebar overlay */}
        {sidebarOpen && (
          <div
            className="fixed inset-0 bg-black/40 z-30 md:hidden"
            onClick={() => setSidebarOpen(false)}
          />
        )}
        <div className={`${
          sidebarOpen ? 'translate-x-0' : '-translate-x-full'
        } fixed md:relative md:translate-x-0 z-30 h-full transition-transform duration-200`}>
          <Sidebar onOpenSettings={() => setSettings(true)} />
        </div>

        <main className="flex-1 flex flex-col min-w-0">
          <MotdBanner />
          <TopBar
            onToggleSidebar={() => setSidebarOpen(!sidebarOpen)}
            onToggleMembers={() => setMembersOpen(!membersOpen)}
            membersOpen={membersOpen}
          />
          <MessageList />
          <ComposeBox />
        </main>
        {/* Member list — inline on desktop, overlay on mobile */}
        {membersOpen && (
          <>
            <div
              className="fixed inset-0 bg-black/40 z-30 md:hidden"
              onClick={() => setMembersOpen(false)}
            />
            <div className="fixed right-0 top-0 bottom-0 z-30 md:relative md:z-auto">
              <MemberList />
            </div>
          </>
        )}
        {threadMsgId && threadChannel && (
          <ThreadView
            rootMsgId={threadMsgId}
            channel={threadChannel}
            onClose={() => useStore.getState().closeThread()}
          />
        )}
      </div>
      <QuickSwitcher open={quickSwitcher} onClose={() => setQuickSwitcher(false)} />
      <SettingsPanel open={settings} onClose={() => setSettings(false)} />
      <ImageLightbox />
      <SearchModal />
      <ChannelListModal />
      <JoinGateModal />
      <ChannelSettingsPanel />
      <KeyboardShortcuts open={shortcuts} onClose={() => setShortcuts(false)} />
      <ToastContainer />
      <FileDropOverlay />
      <InstallPrompt />
      <OnboardingTour />
      <BookmarksPanel />
    </div>
  );
}
