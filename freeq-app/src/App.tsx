import { useState, useCallback, useEffect } from 'react';
import { useStore } from './store';
import { useKeyboard } from './hooks/useKeyboard';
import { setUnreadCount, requestPermission } from './lib/notifications';
import { ConnectScreen } from './components/ConnectScreen';
import { Sidebar } from './components/Sidebar';
import { TopBar } from './components/TopBar';
import { MessageList } from './components/MessageList';
import { ComposeBox } from './components/ComposeBox';
import { MemberList } from './components/MemberList';
import { QuickSwitcher } from './components/QuickSwitcher';
import { SettingsPanel } from './components/SettingsPanel';
import { ReconnectBanner } from './components/ReconnectBanner';
import { ImageLightbox } from './components/ImageLightbox';
import { SearchModal } from './components/SearchModal';
import { ChannelListModal } from './components/ChannelListModal';
import { ThreadView } from './components/ThreadView';

export default function App() {
  const registered = useStore((s) => s.registered);
  const theme = useStore((s) => s.theme);
  const [quickSwitcher, setQuickSwitcher] = useState(false);
  const [settings, setSettings] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [membersOpen, setMembersOpen] = useState(true);
  const threadMsgId = useStore((s) => s.threadMsgId);
  const threadChannel = useStore((s) => s.threadChannel);
  const channels = useStore((s) => s.channels);
  const activeChannel = useStore((s) => s.activeChannel);
  const setActive = useStore((s) => s.setActiveChannel);

  // Apply theme to document
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
  }, [theme]);

  // Request notification permission when registered
  useEffect(() => {
    if (registered) requestPermission();
  }, [registered]);

  // Close sidebar on mobile when switching channels
  useEffect(() => {
    if (window.innerWidth < 768) setSidebarOpen(false);
  }, [activeChannel]);

  // Total unread for title badge
  const totalUnread = [...channels.values()].reduce((sum, ch) => sum + ch.unreadCount, 0);
  setUnreadCount(totalUnread);

  // Global keyboard shortcuts
  const switchToNth = useCallback((n: number) => {
    const sorted = [...channels.values()]
      .filter((ch) => ch.isJoined)
      .sort((a, b) => a.name.localeCompare(b.name));
    if (sorted[n]) setActive(sorted[n].name);
  }, [channels, setActive]);

  useKeyboard({
    'mod+k': () => setQuickSwitcher(true),
    'mod+f': () => useStore.getState().setSearchOpen(true),
    'mod+1': () => switchToNth(0),
    'mod+2': () => switchToNth(1),
    'mod+3': () => switchToNth(2),
    'mod+4': () => switchToNth(3),
    'mod+5': () => switchToNth(4),
    'mod+6': () => switchToNth(5),
    'mod+7': () => switchToNth(6),
    'mod+8': () => switchToNth(7),
    'mod+9': () => switchToNth(8),
    'escape': () => {
      setQuickSwitcher(false);
      setSettings(false);
      useStore.getState().setSearchOpen(false);
      useStore.getState().setChannelListOpen(false);
      useStore.getState().setLightboxUrl(null);
      useStore.getState().closeThread();
    },
  }, [channels, switchToNth]);

  if (!registered) {
    return (
      <div className="h-dvh flex flex-col bg-bg">
        <ConnectScreen />
      </div>
    );
  }

  return (
    <div className="h-dvh flex flex-col bg-bg">
      <ReconnectBanner />
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
          <TopBar
            onToggleSidebar={() => setSidebarOpen(!sidebarOpen)}
            onToggleMembers={() => setMembersOpen(!membersOpen)}
            membersOpen={membersOpen}
          />
          <MessageList />
          <ComposeBox />
        </main>
        {membersOpen && <MemberList />}
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
    </div>
  );
}
