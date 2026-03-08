import Foundation
import SQLite3

/// Local SQLite message store for persistence across app restarts.
actor MessageStore {
    static let shared = MessageStore()
    private var db: OpaquePointer?

    init() {
        openDatabase()
        createTable()
    }

    private func openDatabase() {
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        let dir = caches.appendingPathComponent("at.freeq.macos", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let dbPath = dir.appendingPathComponent("messages.sqlite").path

        if sqlite3_open(dbPath, &db) != SQLITE_OK {
            Log.irc.error("Failed to open message database")
        }
        // WAL mode for performance
        sqlite3_exec(db, "PRAGMA journal_mode=WAL;", nil, nil, nil)
    }

    private func createTable() {
        let sql = """
        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            channel TEXT NOT NULL,
            from_nick TEXT NOT NULL,
            text TEXT NOT NULL,
            timestamp REAL NOT NULL,
            is_action INTEGER DEFAULT 0,
            is_signed INTEGER DEFAULT 0,
            is_edited INTEGER DEFAULT 0,
            is_deleted INTEGER DEFAULT 0,
            reply_to TEXT,
            UNIQUE(id)
        );
        CREATE INDEX IF NOT EXISTS idx_messages_channel ON messages(channel, timestamp);
        CREATE INDEX IF NOT EXISTS idx_messages_timestamp ON messages(timestamp);
        """
        sqlite3_exec(db, sql, nil, nil, nil)
    }

    /// Store a message.
    func store(_ msg: ChatMessage, channel: String) {
        let sql = """
        INSERT OR REPLACE INTO messages (id, channel, from_nick, text, timestamp, is_action, is_signed, is_edited, is_deleted, reply_to)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }

        sqlite3_bind_text(stmt, 1, (msg.id as NSString).utf8String, -1, nil)
        sqlite3_bind_text(stmt, 2, (channel.lowercased() as NSString).utf8String, -1, nil)
        sqlite3_bind_text(stmt, 3, (msg.from as NSString).utf8String, -1, nil)
        sqlite3_bind_text(stmt, 4, (msg.text as NSString).utf8String, -1, nil)
        sqlite3_bind_double(stmt, 5, msg.timestamp.timeIntervalSince1970)
        sqlite3_bind_int(stmt, 6, msg.isAction ? 1 : 0)
        sqlite3_bind_int(stmt, 7, msg.isSigned ? 1 : 0)
        sqlite3_bind_int(stmt, 8, msg.isEdited ? 1 : 0)
        sqlite3_bind_int(stmt, 9, msg.isDeleted ? 1 : 0)
        sqlite3_bind_text(stmt, 10, ((msg.replyTo ?? "") as NSString).utf8String, -1, nil)
        sqlite3_step(stmt)
    }

    /// Load recent messages for a channel.
    func loadMessages(channel: String, limit: Int = 200) -> [ChatMessage] {
        let sql = """
        SELECT id, from_nick, text, timestamp, is_action, is_signed, is_edited, is_deleted, reply_to
        FROM messages
        WHERE channel = ? AND is_deleted = 0
        ORDER BY timestamp DESC
        LIMIT ?
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(stmt) }

        sqlite3_bind_text(stmt, 1, (channel.lowercased() as NSString).utf8String, -1, nil)
        sqlite3_bind_int(stmt, 2, Int32(limit))

        var messages: [ChatMessage] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            let id = String(cString: sqlite3_column_text(stmt, 0))
            let from = String(cString: sqlite3_column_text(stmt, 1))
            let text = String(cString: sqlite3_column_text(stmt, 2))
            let timestamp = Date(timeIntervalSince1970: sqlite3_column_double(stmt, 3))
            let isAction = sqlite3_column_int(stmt, 4) != 0
            let isSigned = sqlite3_column_int(stmt, 5) != 0
            let isEdited = sqlite3_column_int(stmt, 6) != 0
            let replyTo = sqlite3_column_text(stmt, 8).map(String.init(cString:))

            var msg = ChatMessage(
                id: id, from: from, text: text, isAction: isAction,
                timestamp: timestamp, replyTo: replyTo?.isEmpty == true ? nil : replyTo
            )
            msg.isSigned = isSigned
            msg.isEdited = isEdited
            messages.append(msg)
        }
        return messages.reversed()  // Oldest first
    }

    /// Mark a message as deleted.
    func markDeleted(msgId: String) {
        let sql = "UPDATE messages SET is_deleted = 1 WHERE id = ?"
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, (msgId as NSString).utf8String, -1, nil)
        sqlite3_step(stmt)
    }

    /// Update edited message.
    func markEdited(msgId: String, newText: String) {
        let sql = "UPDATE messages SET text = ?, is_edited = 1 WHERE id = ?"
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        sqlite3_bind_text(stmt, 1, (newText as NSString).utf8String, -1, nil)
        sqlite3_bind_text(stmt, 2, (msgId as NSString).utf8String, -1, nil)
        sqlite3_step(stmt)
    }

    /// Search messages.
    func search(query: String, channel: String? = nil, limit: Int = 50) -> [(channel: String, msg: ChatMessage)] {
        var sql = """
        SELECT id, channel, from_nick, text, timestamp, is_action, is_signed
        FROM messages
        WHERE is_deleted = 0 AND (text LIKE ? OR from_nick LIKE ?)
        """
        if channel != nil { sql += " AND channel = ?" }
        sql += " ORDER BY timestamp DESC LIMIT ?"

        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(stmt) }

        let pattern = "%\(query)%"
        sqlite3_bind_text(stmt, 1, (pattern as NSString).utf8String, -1, nil)
        sqlite3_bind_text(stmt, 2, (pattern as NSString).utf8String, -1, nil)
        var paramIdx: Int32 = 3
        if let ch = channel {
            sqlite3_bind_text(stmt, paramIdx, (ch.lowercased() as NSString).utf8String, -1, nil)
            paramIdx += 1
        }
        sqlite3_bind_int(stmt, paramIdx, Int32(limit))

        var results: [(String, ChatMessage)] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            let id = String(cString: sqlite3_column_text(stmt, 0))
            let ch = String(cString: sqlite3_column_text(stmt, 1))
            let from = String(cString: sqlite3_column_text(stmt, 2))
            let text = String(cString: sqlite3_column_text(stmt, 3))
            let timestamp = Date(timeIntervalSince1970: sqlite3_column_double(stmt, 4))
            let isAction = sqlite3_column_int(stmt, 5) != 0
            var msg = ChatMessage(id: id, from: from, text: text, isAction: isAction, timestamp: timestamp, replyTo: nil)
            msg.isSigned = sqlite3_column_int(stmt, 6) != 0
            results.append((ch, msg))
        }
        return results
    }
}
