import SwiftUI
import Cdk

// MARK: - Clipboard Helpers

func copyToClipboard(_ string: String) {
    #if canImport(UIKit)
    UIPasteboard.general.string = string
    #elseif canImport(AppKit)
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(string, forType: .string)
    #endif
}

func pasteFromClipboard() -> String {
    #if canImport(UIKit)
    return UIPasteboard.general.string ?? ""
    #elseif canImport(AppKit)
    return NSPasteboard.general.string(forType: .string) ?? ""
    #endif
}

// MARK: - View Model

@Observable
@MainActor
final class WalletViewModel {
    // Connection
    var mintUrl = "https://testnut.cashu.space"
    var isConnected = false
    var isConnecting = false

    // Balance
    var balance: UInt64 = 0
    var pendingBalance: UInt64 = 0
    var isRefreshing = false

    // Mint
    var mintAmount = ""
    var mintQuote: MintQuote?
    var isMinting = false
    var mintSuccess = false

    // Send
    var sendAmount = ""
    var generatedToken = ""
    var isSending = false

    // Receive
    var receiveToken = ""
    var receivedAmount: UInt64?
    var isReceiving = false

    // Melt
    var meltInvoice = ""
    var meltQuote: MeltQuote?
    var isMelting = false
    var meltSuccess = false

    // Transactions
    var transactions: [Cdk.Transaction] = []

    // General
    var errorMessage: String?

    private var wallet: Wallet?

    // MARK: - Connect

    func connect() async {
        isConnecting = true
        errorMessage = nil
        do {
            initDefaultLogging()
            let mnemonic = try generateMnemonic()
            let dbPath = FileManager.default.temporaryDirectory
                .appendingPathComponent("cdk_swift_\(UUID().uuidString).db")
                .path
            wallet = try Wallet(
                mintUrl: mintUrl,
                unit: .sat,
                mnemonic: mnemonic,
                store: .sqlite(path: dbPath),
                config: WalletConfig(targetProofCount: nil)
            )
            isConnected = true
        } catch {
            errorMessage = "Connection failed: \(error.localizedDescription)"
        }
        isConnecting = false
    }

    // MARK: - Balance

    func refreshBalance() async {
        guard let wallet else { return }
        isRefreshing = true
        do {
            balance = try await wallet.totalBalance().value
            pendingBalance = try await wallet.totalPendingBalance().value
        } catch {
            errorMessage = "Balance error: \(error.localizedDescription)"
        }
        isRefreshing = false
    }

    // MARK: - Mint

    func createMintQuote() async {
        guard let wallet, let amount = UInt64(mintAmount), amount > 0 else {
            errorMessage = "Enter a valid amount"
            return
        }
        isMinting = true
        errorMessage = nil
        mintSuccess = false
        do {
            mintQuote = try await wallet.mintQuote(
                paymentMethod: .bolt11,
                amount: Amount(value: amount),
                description: nil,
                extra: nil
            )
        } catch {
            errorMessage = "Mint quote error: \(error.localizedDescription)"
        }
        isMinting = false
    }

    func mintTokens() async {
        guard let wallet, let quote = mintQuote else { return }
        isMinting = true
        errorMessage = nil
        do {
            _ = try await wallet.mint(
                quoteId: quote.id,
                amountSplitTarget: .none,
                spendingConditions: nil
            )
            mintSuccess = true
            mintQuote = nil
            mintAmount = ""
            await refreshBalance()
        } catch {
            errorMessage = "Mint error: \(error.localizedDescription)"
        }
        isMinting = false
    }

    // MARK: - Send

    func send() async {
        guard let wallet, let amount = UInt64(sendAmount), amount > 0 else {
            errorMessage = "Enter a valid amount"
            return
        }
        isSending = true
        errorMessage = nil
        generatedToken = ""
        do {
            let options = SendOptions(
                memo: nil,
                conditions: nil,
                amountSplitTarget: .none,
                sendKind: .onlineExact,
                includeFee: false,
                useP2bk: false,
                maxProofs: nil,
                metadata: [:]
            )
            let prepared = try await wallet.prepareSend(
                amount: Amount(value: amount),
                options: options
            )
            let token = try await prepared.confirm(memo: nil)
            generatedToken = token.encode()
            sendAmount = ""
            await refreshBalance()
        } catch {
            errorMessage = "Send error: \(error.localizedDescription)"
        }
        isSending = false
    }

    // MARK: - Receive

    func receive() async {
        guard let wallet, !receiveToken.isEmpty else {
            errorMessage = "Paste a Cashu token"
            return
        }
        isReceiving = true
        errorMessage = nil
        receivedAmount = nil
        do {
            let token = try Token.decode(encodedToken: receiveToken)
            let options = ReceiveOptions(
                amountSplitTarget: .none,
                p2pkSigningKeys: [],
                preimages: [],
                metadata: [:]
            )
            let amount = try await wallet.receive(token: token, options: options)
            receivedAmount = amount.value
            receiveToken = ""
            await refreshBalance()
        } catch {
            errorMessage = "Receive error: \(error.localizedDescription)"
        }
        isReceiving = false
    }

    // MARK: - Melt

    func createMeltQuote() async {
        guard let wallet, !meltInvoice.isEmpty else {
            errorMessage = "Paste a Lightning invoice"
            return
        }
        isMelting = true
        errorMessage = nil
        meltSuccess = false
        do {
            meltQuote = try await wallet.meltQuote(
                method: .bolt11,
                request: meltInvoice,
                options: nil,
                extra: nil
            )
        } catch {
            errorMessage = "Melt quote error: \(error.localizedDescription)"
        }
        isMelting = false
    }

    func confirmMelt() async {
        guard let wallet, let quote = meltQuote else { return }
        isMelting = true
        errorMessage = nil
        do {
            let prepared = try await wallet.prepareMelt(quoteId: quote.id)
            _ = try await prepared.confirm()
            meltSuccess = true
            meltQuote = nil
            meltInvoice = ""
            await refreshBalance()
        } catch {
            errorMessage = "Melt error: \(error.localizedDescription)"
        }
        isMelting = false
    }

    // MARK: - Transactions

    func loadTransactions() async {
        guard let wallet else { return }
        do {
            transactions = try await wallet.listTransactions(direction: nil)
        } catch {
            errorMessage = "Transactions error: \(error.localizedDescription)"
        }
    }
}

// MARK: - App

@main
struct CdkExampleApp: App {
    @State private var vm = WalletViewModel()

    init() {
        #if os(macOS)
        NSApplication.shared.setActivationPolicy(.regular)
        NSApplication.shared.activate(ignoringOtherApps: true)
        #endif
    }

    var body: some Scene {
        WindowGroup {
            ContentView(vm: vm)
        }
    }
}

// MARK: - Content View

struct ContentView: View {
    @Bindable var vm: WalletViewModel

    var body: some View {
        Group {
            if vm.isConnected {
                MainTabView(vm: vm)
            } else {
                SetupView(vm: vm)
            }
        }
    }
}

// MARK: - Setup View

struct SetupView: View {
    @Bindable var vm: WalletViewModel

    var body: some View {
        VStack(spacing: 20) {
            Text("CDK Wallet")
                .font(.largeTitle.bold())

            Text("Enter a Cashu mint URL to get started")
                .foregroundStyle(.secondary)

            TextField("Mint URL", text: $vm.mintUrl)
                .textFieldStyle(.roundedBorder)
                #if os(iOS)
                .textInputAutocapitalization(.never)
                .keyboardType(.URL)
                #endif

            Button {
                Task { await vm.connect() }
            } label: {
                if vm.isConnecting {
                    ProgressView()
                        .frame(maxWidth: .infinity)
                } else {
                    Text("Connect")
                        .frame(maxWidth: .infinity)
                }
            }
            .buttonStyle(.borderedProminent)
            .disabled(vm.mintUrl.isEmpty || vm.isConnecting)

            if let error = vm.errorMessage {
                Text(error)
                    .foregroundStyle(.red)
                    .font(.caption)
            }
        }
        .padding(40)
        .frame(maxWidth: 400)
    }
}

// MARK: - Main Tab View

struct MainTabView: View {
    @Bindable var vm: WalletViewModel

    var body: some View {
        TabView {
            BalanceView(vm: vm)
                .tabItem { Label("Balance", systemImage: "bitcoinsign.circle") }
            MintView(vm: vm)
                .tabItem { Label("Mint", systemImage: "plus.circle") }
            SendView(vm: vm)
                .tabItem { Label("Send", systemImage: "arrow.up.circle") }
            ReceiveView(vm: vm)
                .tabItem { Label("Receive", systemImage: "arrow.down.circle") }
            MeltView(vm: vm)
                .tabItem { Label("Melt", systemImage: "bolt.circle") }
            TransactionListView(vm: vm)
                .tabItem { Label("History", systemImage: "list.bullet") }
        }
        .task { await vm.refreshBalance() }
    }
}

// MARK: - Balance View

struct BalanceView: View {
    @Bindable var vm: WalletViewModel

    var body: some View {
        VStack(spacing: 16) {
            Spacer()

            Text("\(vm.balance)")
                .font(.system(size: 64, weight: .bold, design: .rounded))
            Text("sats")
                .font(.title2)
                .foregroundStyle(.secondary)

            if vm.pendingBalance > 0 {
                Text("\(vm.pendingBalance) sats pending")
                    .foregroundStyle(.orange)
            }

            Spacer()

            Button {
                Task { await vm.refreshBalance() }
            } label: {
                if vm.isRefreshing {
                    ProgressView()
                } else {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }
            }
            .buttonStyle(.bordered)
            .disabled(vm.isRefreshing)

            Text(vm.mintUrl)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding()
        .navigationTitle("Balance")
    }
}

// MARK: - Mint View

struct MintView: View {
    @Bindable var vm: WalletViewModel

    var body: some View {
        ScrollView {
            VStack(spacing: 16) {
                if vm.mintSuccess {
                    Label("Tokens minted successfully!", systemImage: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                        .font(.headline)
                }

                if let quote = vm.mintQuote {
                    VStack(spacing: 12) {
                        Text("Pay this Lightning invoice:")
                            .font(.headline)

                        Text(quote.request)
                            .font(.system(.caption, design: .monospaced))
                            .textSelection(.enabled)
                            .padding(8)
                            .background(Color.gray.opacity(0.1))
                            .cornerRadius(8)

                        Button("Copy Invoice") {
                            copyToClipboard(quote.request)
                        }
                        .buttonStyle(.bordered)

                        Button {
                            Task { await vm.mintTokens() }
                        } label: {
                            if vm.isMinting {
                                ProgressView()
                                    .frame(maxWidth: .infinity)
                            } else {
                                Text("I've Paid - Mint Tokens")
                                    .frame(maxWidth: .infinity)
                            }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(vm.isMinting)
                    }
                } else {
                    TextField("Amount (sats)", text: $vm.mintAmount)
                        .textFieldStyle(.roundedBorder)
                        #if os(iOS)
                        .keyboardType(.numberPad)
                        #endif

                    Button {
                        Task { await vm.createMintQuote() }
                    } label: {
                        if vm.isMinting {
                            ProgressView()
                                .frame(maxWidth: .infinity)
                        } else {
                            Text("Get Invoice")
                                .frame(maxWidth: .infinity)
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(vm.mintAmount.isEmpty || vm.isMinting)
                }

                if let error = vm.errorMessage {
                    Text(error)
                        .foregroundStyle(.red)
                        .font(.caption)
                }
            }
            .padding()
        }
        .navigationTitle("Mint")
    }
}

// MARK: - Send View

struct SendView: View {
    @Bindable var vm: WalletViewModel

    var body: some View {
        ScrollView {
            VStack(spacing: 16) {
                if !vm.generatedToken.isEmpty {
                    VStack(spacing: 12) {
                        Label("Token Created", systemImage: "checkmark.circle.fill")
                            .foregroundStyle(.green)
                            .font(.headline)

                        Text(vm.generatedToken)
                            .font(.system(.caption, design: .monospaced))
                            .textSelection(.enabled)
                            .padding(8)
                            .background(Color.gray.opacity(0.1))
                            .cornerRadius(8)
                            .frame(maxHeight: 120)

                        Button("Copy Token") {
                            copyToClipboard(vm.generatedToken)
                        }
                        .buttonStyle(.borderedProminent)
                    }
                }

                TextField("Amount (sats)", text: $vm.sendAmount)
                    .textFieldStyle(.roundedBorder)
                    #if os(iOS)
                    .keyboardType(.numberPad)
                    #endif

                Button {
                    Task { await vm.send() }
                } label: {
                    if vm.isSending {
                        ProgressView()
                            .frame(maxWidth: .infinity)
                    } else {
                        Text("Create Token")
                            .frame(maxWidth: .infinity)
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(vm.sendAmount.isEmpty || vm.isSending)

                if let error = vm.errorMessage {
                    Text(error)
                        .foregroundStyle(.red)
                        .font(.caption)
                }
            }
            .padding()
        }
        .navigationTitle("Send")
    }
}

// MARK: - Receive View

struct ReceiveView: View {
    @Bindable var vm: WalletViewModel

    var body: some View {
        ScrollView {
            VStack(spacing: 16) {
                if let amount = vm.receivedAmount {
                    Label("Received \(amount) sats!", systemImage: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                        .font(.headline)
                }

                TextField("Paste Cashu token", text: $vm.receiveToken, axis: .vertical)
                    .textFieldStyle(.roundedBorder)
                    .lineLimit(3...6)

                HStack {
                    Button("Paste") {
                        vm.receiveToken = pasteFromClipboard()
                    }
                    .buttonStyle(.bordered)

                    Button {
                        Task { await vm.receive() }
                    } label: {
                        if vm.isReceiving {
                            ProgressView()
                                .frame(maxWidth: .infinity)
                        } else {
                            Text("Receive")
                                .frame(maxWidth: .infinity)
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(vm.receiveToken.isEmpty || vm.isReceiving)
                }

                if let error = vm.errorMessage {
                    Text(error)
                        .foregroundStyle(.red)
                        .font(.caption)
                }
            }
            .padding()
        }
        .navigationTitle("Receive")
    }
}

// MARK: - Melt View

struct MeltView: View {
    @Bindable var vm: WalletViewModel

    var body: some View {
        ScrollView {
            VStack(spacing: 16) {
                if vm.meltSuccess {
                    Label("Payment sent!", systemImage: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                        .font(.headline)
                }

                if let quote = vm.meltQuote {
                    VStack(spacing: 12) {
                        Text("Payment Summary")
                            .font(.headline)

                        HStack {
                            Text("Amount:")
                            Spacer()
                            Text("\(quote.amount.value) sats")
                                .bold()
                        }
                        HStack {
                            Text("Fee reserve:")
                            Spacer()
                            Text("\(quote.feeReserve.value) sats")
                        }

                        Button {
                            Task { await vm.confirmMelt() }
                        } label: {
                            if vm.isMelting {
                                ProgressView()
                                    .frame(maxWidth: .infinity)
                            } else {
                                Text("Confirm Payment")
                                    .frame(maxWidth: .infinity)
                            }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(vm.isMelting)

                        Button("Cancel") {
                            vm.meltQuote = nil
                        }
                        .buttonStyle(.bordered)
                    }
                } else {
                    TextField("Paste Lightning invoice", text: $vm.meltInvoice, axis: .vertical)
                        .textFieldStyle(.roundedBorder)
                        .lineLimit(3...6)

                    HStack {
                        Button("Paste") {
                            vm.meltInvoice = pasteFromClipboard()
                        }
                        .buttonStyle(.bordered)

                        Button {
                            Task { await vm.createMeltQuote() }
                        } label: {
                            if vm.isMelting {
                                ProgressView()
                                    .frame(maxWidth: .infinity)
                            } else {
                                Text("Get Quote")
                                    .frame(maxWidth: .infinity)
                            }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(vm.meltInvoice.isEmpty || vm.isMelting)
                    }
                }

                if let error = vm.errorMessage {
                    Text(error)
                        .foregroundStyle(.red)
                        .font(.caption)
                }
            }
            .padding()
        }
        .navigationTitle("Melt")
    }
}

// MARK: - Transaction List View

struct TransactionListView: View {
    var vm: WalletViewModel

    var body: some View {
        List {
            if vm.transactions.isEmpty {
                ContentUnavailableView(
                    "No Transactions",
                    systemImage: "tray",
                    description: Text("Transactions will appear here")
                )
            } else {
                ForEach(Array(vm.transactions.enumerated()), id: \.offset) { _, tx in
                    let isIncoming = tx.direction == .incoming
                    HStack {
                        Image(systemName: isIncoming
                              ? "arrow.down.circle.fill"
                              : "arrow.up.circle.fill")
                            .foregroundStyle(isIncoming ? .green : .orange)

                        VStack(alignment: .leading) {
                            Text(isIncoming ? "Incoming" : "Outgoing")
                                .font(.headline)
                            Text(String(describing: tx.id))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                        }

                        Spacer()

                        Text("\(tx.amount.value) sats")
                            .font(.headline.monospacedDigit())
                    }
                    .padding(.vertical, 4)
                }
            }
        }
        .refreshable { await vm.loadTransactions() }
        .task { await vm.loadTransactions() }
        .navigationTitle("History")
    }
}
