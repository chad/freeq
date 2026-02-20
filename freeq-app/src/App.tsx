import { useStore } from './store';
import { ConnectScreen } from './components/ConnectScreen';
import { Sidebar } from './components/Sidebar';
import { TopBar } from './components/TopBar';
import { MessageList } from './components/MessageList';
import { ComposeBox } from './components/ComposeBox';
import { MemberList } from './components/MemberList';

export default function App() {
  const registered = useStore((s) => s.registered);

  if (!registered) {
    return (
      <div className="h-screen flex flex-col">
        <ConnectScreen />
      </div>
    );
  }

  return (
    <div className="h-screen flex flex-col">
      <TopBar />
      <div className="flex flex-1 min-h-0">
        <Sidebar />
        <main className="flex-1 flex flex-col min-w-0">
          <MessageList />
          <ComposeBox />
        </main>
        <MemberList />
      </div>
    </div>
  );
}
