// briardr-transcribe: on-device speech-to-text for Briardr's hold-to-dictate.
// Usage: briardr-transcribe <audio-file> [locale]
// Uses SpeechAnalyzer/SpeechTranscriber (macOS 26), which runs entirely on this Mac.
// Prints the transcript to stdout. Exit codes: 0 ok, 2 unsupported locale, 3 model unavailable, 4 failed.
import AVFoundation
import Foundation
import Speech

@main
struct Transcribe {
    static func main() async {
        let args = CommandLine.arguments
        guard args.count >= 2 else {
            FileHandle.standardError.write("usage: briardr-transcribe <audio-file> [locale]\n".data(using: .utf8)!)
            exit(64)
        }
        let url = URL(fileURLWithPath: args[1])
        let wanted = args.count >= 3 ? Locale(identifier: args[2]) : Locale.current
        do {
            var match = await SpeechTranscriber.supportedLocale(equivalentTo: wanted)
            if match == nil { match = await SpeechTranscriber.supportedLocale(equivalentTo: Locale(identifier: "en-US")) }
            guard let locale = match else {
                FileHandle.standardError.write("unsupported_locale\n".data(using: .utf8)!)
                exit(2)
            }
            let transcriber = SpeechTranscriber(locale: locale, preset: .transcription)
            if let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
                try await request.downloadAndInstall()
            }
            let analyzer = SpeechAnalyzer(modules: [transcriber])
            let file = try AVAudioFile(forReading: url)
            async let text: String = transcriber.results.reduce(into: "") { acc, r in
                if r.isFinal { acc += String(r.text.characters) }
            }
            if let last = try await analyzer.analyzeSequence(from: file) {
                try await analyzer.finalizeAndFinish(through: last)
            } else {
                await analyzer.cancelAndFinishNow()
            }
            let result = try await text
            print(result.trimmingCharacters(in: .whitespacesAndNewlines))
        } catch {
            FileHandle.standardError.write("failed: \(error.localizedDescription)\n".data(using: .utf8)!)
            exit(4)
        }
    }
}
