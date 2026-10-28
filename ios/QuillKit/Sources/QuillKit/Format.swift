import Foundation

/// Numbers as the desktop shows them.
public enum Format {
    /// "1 word", "1,234 words".
    public static func words(_ count: UInt64) -> String {
        let number = count.formatted(.number.grouping(.automatic).locale(Locale(identifier: "en_US")))
        return count == 1 ? "1 word" : "\(number) words"
    }
}
