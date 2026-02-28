library cdk;

import "dart:async";
import "dart:convert";
import "dart:ffi";
import "dart:io" show Platform, File, Directory;
import "dart:isolate";
import "dart:typed_data";
import "package:ffi/ffi.dart";

class Amount {
  final int value;
  Amount({required this.value});
}

class FfiConverterAmount {
  static Amount lift(RustBuffer buf) {
    return FfiConverterAmount.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Amount> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final value_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final value = value_lifted.value;
    new_offset += value_lifted.bytesRead;
    return LiftRetVal(Amount(value: value), new_offset - buf.offsetInBytes);
  }

  static RustBuffer lower(Amount value) {
    final total_length = FfiConverterUInt64.allocationSize(value.value) + 0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(Amount value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterUInt64.write(
      value.value,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(Amount value) {
    return FfiConverterUInt64.allocationSize(value.value) + 0;
  }
}

class AuthProof {
  final String keysetId;
  final String secret;
  final String c;
  final String y;
  AuthProof({
    required this.keysetId,
    required this.secret,
    required this.c,
    required this.y,
  });
}

class FfiConverterAuthProof {
  static AuthProof lift(RustBuffer buf) {
    return FfiConverterAuthProof.read(buf.asUint8List()).value;
  }

  static LiftRetVal<AuthProof> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final keysetId_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final keysetId = keysetId_lifted.value;
    new_offset += keysetId_lifted.bytesRead;
    final secret_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final secret = secret_lifted.value;
    new_offset += secret_lifted.bytesRead;
    final c_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final c = c_lifted.value;
    new_offset += c_lifted.bytesRead;
    final y_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final y = y_lifted.value;
    new_offset += y_lifted.bytesRead;
    return LiftRetVal(
      AuthProof(keysetId: keysetId, secret: secret, c: c, y: y),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(AuthProof value) {
    final total_length =
        FfiConverterString.allocationSize(value.keysetId) +
        FfiConverterString.allocationSize(value.secret) +
        FfiConverterString.allocationSize(value.c) +
        FfiConverterString.allocationSize(value.y) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(AuthProof value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.keysetId,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.secret,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.c,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.y,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(AuthProof value) {
    return FfiConverterString.allocationSize(value.keysetId) +
        FfiConverterString.allocationSize(value.secret) +
        FfiConverterString.allocationSize(value.c) +
        FfiConverterString.allocationSize(value.y) +
        0;
  }
}

class BackupOptions {
  final String? client;
  BackupOptions({required this.client});
}

class FfiConverterBackupOptions {
  static BackupOptions lift(RustBuffer buf) {
    return FfiConverterBackupOptions.read(buf.asUint8List()).value;
  }

  static LiftRetVal<BackupOptions> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final client_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final client = client_lifted.value;
    new_offset += client_lifted.bytesRead;
    return LiftRetVal(
      BackupOptions(client: client),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(BackupOptions value) {
    final total_length =
        FfiConverterOptionalString.allocationSize(value.client) + 0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(BackupOptions value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterOptionalString.write(
      value.client,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(BackupOptions value) {
    return FfiConverterOptionalString.allocationSize(value.client) + 0;
  }
}

class BackupResult {
  final String eventId;
  final String publicKey;
  final int mintCount;
  BackupResult({
    required this.eventId,
    required this.publicKey,
    required this.mintCount,
  });
}

class FfiConverterBackupResult {
  static BackupResult lift(RustBuffer buf) {
    return FfiConverterBackupResult.read(buf.asUint8List()).value;
  }

  static LiftRetVal<BackupResult> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final eventId_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final eventId = eventId_lifted.value;
    new_offset += eventId_lifted.bytesRead;
    final publicKey_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final publicKey = publicKey_lifted.value;
    new_offset += publicKey_lifted.bytesRead;
    final mintCount_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final mintCount = mintCount_lifted.value;
    new_offset += mintCount_lifted.bytesRead;
    return LiftRetVal(
      BackupResult(
        eventId: eventId,
        publicKey: publicKey,
        mintCount: mintCount,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(BackupResult value) {
    final total_length =
        FfiConverterString.allocationSize(value.eventId) +
        FfiConverterString.allocationSize(value.publicKey) +
        FfiConverterUInt64.allocationSize(value.mintCount) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(BackupResult value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.eventId,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.publicKey,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.mintCount,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(BackupResult value) {
    return FfiConverterString.allocationSize(value.eventId) +
        FfiConverterString.allocationSize(value.publicKey) +
        FfiConverterUInt64.allocationSize(value.mintCount) +
        0;
  }
}

class BlindAuthSettings {
  final int batMaxMint;
  final List<ProtectedEndpoint> protectedEndpoints;
  BlindAuthSettings({
    required this.batMaxMint,
    required this.protectedEndpoints,
  });
}

class FfiConverterBlindAuthSettings {
  static BlindAuthSettings lift(RustBuffer buf) {
    return FfiConverterBlindAuthSettings.read(buf.asUint8List()).value;
  }

  static LiftRetVal<BlindAuthSettings> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final batMaxMint_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final batMaxMint = batMaxMint_lifted.value;
    new_offset += batMaxMint_lifted.bytesRead;
    final protectedEndpoints_lifted =
        FfiConverterSequenceProtectedEndpoint.read(
          Uint8List.view(buf.buffer, new_offset),
        );
    final protectedEndpoints = protectedEndpoints_lifted.value;
    new_offset += protectedEndpoints_lifted.bytesRead;
    return LiftRetVal(
      BlindAuthSettings(
        batMaxMint: batMaxMint,
        protectedEndpoints: protectedEndpoints,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(BlindAuthSettings value) {
    final total_length =
        FfiConverterUInt64.allocationSize(value.batMaxMint) +
        FfiConverterSequenceProtectedEndpoint.allocationSize(
          value.protectedEndpoints,
        ) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(BlindAuthSettings value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterUInt64.write(
      value.batMaxMint,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSequenceProtectedEndpoint.write(
      value.protectedEndpoints,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(BlindAuthSettings value) {
    return FfiConverterUInt64.allocationSize(value.batMaxMint) +
        FfiConverterSequenceProtectedEndpoint.allocationSize(
          value.protectedEndpoints,
        ) +
        0;
  }
}

class BlindSignatureDleq {
  final String e;
  final String s;
  BlindSignatureDleq({required this.e, required this.s});
}

class FfiConverterBlindSignatureDleq {
  static BlindSignatureDleq lift(RustBuffer buf) {
    return FfiConverterBlindSignatureDleq.read(buf.asUint8List()).value;
  }

  static LiftRetVal<BlindSignatureDleq> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final e_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final e = e_lifted.value;
    new_offset += e_lifted.bytesRead;
    final s_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final s = s_lifted.value;
    new_offset += s_lifted.bytesRead;
    return LiftRetVal(
      BlindSignatureDleq(e: e, s: s),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(BlindSignatureDleq value) {
    final total_length =
        FfiConverterString.allocationSize(value.e) +
        FfiConverterString.allocationSize(value.s) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(BlindSignatureDleq value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.e,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.s,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(BlindSignatureDleq value) {
    return FfiConverterString.allocationSize(value.e) +
        FfiConverterString.allocationSize(value.s) +
        0;
  }
}

class ClearAuthSettings {
  final String openidDiscovery;
  final String clientId;
  final List<ProtectedEndpoint> protectedEndpoints;
  ClearAuthSettings({
    required this.openidDiscovery,
    required this.clientId,
    required this.protectedEndpoints,
  });
}

class FfiConverterClearAuthSettings {
  static ClearAuthSettings lift(RustBuffer buf) {
    return FfiConverterClearAuthSettings.read(buf.asUint8List()).value;
  }

  static LiftRetVal<ClearAuthSettings> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final openidDiscovery_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final openidDiscovery = openidDiscovery_lifted.value;
    new_offset += openidDiscovery_lifted.bytesRead;
    final clientId_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final clientId = clientId_lifted.value;
    new_offset += clientId_lifted.bytesRead;
    final protectedEndpoints_lifted =
        FfiConverterSequenceProtectedEndpoint.read(
          Uint8List.view(buf.buffer, new_offset),
        );
    final protectedEndpoints = protectedEndpoints_lifted.value;
    new_offset += protectedEndpoints_lifted.bytesRead;
    return LiftRetVal(
      ClearAuthSettings(
        openidDiscovery: openidDiscovery,
        clientId: clientId,
        protectedEndpoints: protectedEndpoints,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(ClearAuthSettings value) {
    final total_length =
        FfiConverterString.allocationSize(value.openidDiscovery) +
        FfiConverterString.allocationSize(value.clientId) +
        FfiConverterSequenceProtectedEndpoint.allocationSize(
          value.protectedEndpoints,
        ) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(ClearAuthSettings value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.openidDiscovery,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.clientId,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSequenceProtectedEndpoint.write(
      value.protectedEndpoints,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(ClearAuthSettings value) {
    return FfiConverterString.allocationSize(value.openidDiscovery) +
        FfiConverterString.allocationSize(value.clientId) +
        FfiConverterSequenceProtectedEndpoint.allocationSize(
          value.protectedEndpoints,
        ) +
        0;
  }
}

class Conditions {
  final int? locktime;
  final List<String> pubkeys;
  final List<String> refundKeys;
  final int? numSigs;
  final int sigFlag;
  final int? numSigsRefund;
  Conditions({
    required this.locktime,
    required this.pubkeys,
    required this.refundKeys,
    required this.numSigs,
    required this.sigFlag,
    required this.numSigsRefund,
  });
}

class FfiConverterConditions {
  static Conditions lift(RustBuffer buf) {
    return FfiConverterConditions.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Conditions> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final locktime_lifted = FfiConverterOptionalUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final locktime = locktime_lifted.value;
    new_offset += locktime_lifted.bytesRead;
    final pubkeys_lifted = FfiConverterSequenceString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final pubkeys = pubkeys_lifted.value;
    new_offset += pubkeys_lifted.bytesRead;
    final refundKeys_lifted = FfiConverterSequenceString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final refundKeys = refundKeys_lifted.value;
    new_offset += refundKeys_lifted.bytesRead;
    final numSigs_lifted = FfiConverterOptionalUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final numSigs = numSigs_lifted.value;
    new_offset += numSigs_lifted.bytesRead;
    final sigFlag_lifted = FfiConverterUInt8.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final sigFlag = sigFlag_lifted.value;
    new_offset += sigFlag_lifted.bytesRead;
    final numSigsRefund_lifted = FfiConverterOptionalUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final numSigsRefund = numSigsRefund_lifted.value;
    new_offset += numSigsRefund_lifted.bytesRead;
    return LiftRetVal(
      Conditions(
        locktime: locktime,
        pubkeys: pubkeys,
        refundKeys: refundKeys,
        numSigs: numSigs,
        sigFlag: sigFlag,
        numSigsRefund: numSigsRefund,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(Conditions value) {
    final total_length =
        FfiConverterOptionalUInt64.allocationSize(value.locktime) +
        FfiConverterSequenceString.allocationSize(value.pubkeys) +
        FfiConverterSequenceString.allocationSize(value.refundKeys) +
        FfiConverterOptionalUInt64.allocationSize(value.numSigs) +
        FfiConverterUInt8.allocationSize(value.sigFlag) +
        FfiConverterOptionalUInt64.allocationSize(value.numSigsRefund) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(Conditions value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterOptionalUInt64.write(
      value.locktime,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSequenceString.write(
      value.pubkeys,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSequenceString.write(
      value.refundKeys,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalUInt64.write(
      value.numSigs,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt8.write(
      value.sigFlag,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalUInt64.write(
      value.numSigsRefund,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(Conditions value) {
    return FfiConverterOptionalUInt64.allocationSize(value.locktime) +
        FfiConverterSequenceString.allocationSize(value.pubkeys) +
        FfiConverterSequenceString.allocationSize(value.refundKeys) +
        FfiConverterOptionalUInt64.allocationSize(value.numSigs) +
        FfiConverterUInt8.allocationSize(value.sigFlag) +
        FfiConverterOptionalUInt64.allocationSize(value.numSigsRefund) +
        0;
  }
}

class ContactInfo {
  final String method;
  final String info;
  ContactInfo({required this.method, required this.info});
}

class FfiConverterContactInfo {
  static ContactInfo lift(RustBuffer buf) {
    return FfiConverterContactInfo.read(buf.asUint8List()).value;
  }

  static LiftRetVal<ContactInfo> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final method_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final method = method_lifted.value;
    new_offset += method_lifted.bytesRead;
    final info_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final info = info_lifted.value;
    new_offset += info_lifted.bytesRead;
    return LiftRetVal(
      ContactInfo(method: method, info: info),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(ContactInfo value) {
    final total_length =
        FfiConverterString.allocationSize(value.method) +
        FfiConverterString.allocationSize(value.info) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(ContactInfo value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.method,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.info,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(ContactInfo value) {
    return FfiConverterString.allocationSize(value.method) +
        FfiConverterString.allocationSize(value.info) +
        0;
  }
}

class CreateRequestParams {
  final int? amount;
  final String unit;
  final String? description;
  final List<String>? pubkeys;
  final int numSigs;
  final String? hash;
  final String? preimage;
  final String transport;
  final String? httpUrl;
  final List<String>? nostrRelays;
  CreateRequestParams({
    required this.amount,
    required this.unit,
    required this.description,
    required this.pubkeys,
    required this.numSigs,
    required this.hash,
    required this.preimage,
    required this.transport,
    required this.httpUrl,
    required this.nostrRelays,
  });
}

class FfiConverterCreateRequestParams {
  static CreateRequestParams lift(RustBuffer buf) {
    return FfiConverterCreateRequestParams.read(buf.asUint8List()).value;
  }

  static LiftRetVal<CreateRequestParams> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final amount_lifted = FfiConverterOptionalUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amount = amount_lifted.value;
    new_offset += amount_lifted.bytesRead;
    final unit_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final description_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final description = description_lifted.value;
    new_offset += description_lifted.bytesRead;
    final pubkeys_lifted = FfiConverterOptionalSequenceString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final pubkeys = pubkeys_lifted.value;
    new_offset += pubkeys_lifted.bytesRead;
    final numSigs_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final numSigs = numSigs_lifted.value;
    new_offset += numSigs_lifted.bytesRead;
    final hash_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final hash = hash_lifted.value;
    new_offset += hash_lifted.bytesRead;
    final preimage_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final preimage = preimage_lifted.value;
    new_offset += preimage_lifted.bytesRead;
    final transport_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final transport = transport_lifted.value;
    new_offset += transport_lifted.bytesRead;
    final httpUrl_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final httpUrl = httpUrl_lifted.value;
    new_offset += httpUrl_lifted.bytesRead;
    final nostrRelays_lifted = FfiConverterOptionalSequenceString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nostrRelays = nostrRelays_lifted.value;
    new_offset += nostrRelays_lifted.bytesRead;
    return LiftRetVal(
      CreateRequestParams(
        amount: amount,
        unit: unit,
        description: description,
        pubkeys: pubkeys,
        numSigs: numSigs,
        hash: hash,
        preimage: preimage,
        transport: transport,
        httpUrl: httpUrl,
        nostrRelays: nostrRelays,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(CreateRequestParams value) {
    final total_length =
        FfiConverterOptionalUInt64.allocationSize(value.amount) +
        FfiConverterString.allocationSize(value.unit) +
        FfiConverterOptionalString.allocationSize(value.description) +
        FfiConverterOptionalSequenceString.allocationSize(value.pubkeys) +
        FfiConverterUInt64.allocationSize(value.numSigs) +
        FfiConverterOptionalString.allocationSize(value.hash) +
        FfiConverterOptionalString.allocationSize(value.preimage) +
        FfiConverterString.allocationSize(value.transport) +
        FfiConverterOptionalString.allocationSize(value.httpUrl) +
        FfiConverterOptionalSequenceString.allocationSize(value.nostrRelays) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(CreateRequestParams value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterOptionalUInt64.write(
      value.amount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.description,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalSequenceString.write(
      value.pubkeys,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.numSigs,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.hash,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.preimage,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.transport,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.httpUrl,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalSequenceString.write(
      value.nostrRelays,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(CreateRequestParams value) {
    return FfiConverterOptionalUInt64.allocationSize(value.amount) +
        FfiConverterString.allocationSize(value.unit) +
        FfiConverterOptionalString.allocationSize(value.description) +
        FfiConverterOptionalSequenceString.allocationSize(value.pubkeys) +
        FfiConverterUInt64.allocationSize(value.numSigs) +
        FfiConverterOptionalString.allocationSize(value.hash) +
        FfiConverterOptionalString.allocationSize(value.preimage) +
        FfiConverterString.allocationSize(value.transport) +
        FfiConverterOptionalString.allocationSize(value.httpUrl) +
        FfiConverterOptionalSequenceString.allocationSize(value.nostrRelays) +
        0;
  }
}

class CreateRequestResult {
  final PaymentRequest paymentRequest;
  final NostrWaitInfo? nostrWaitInfo;
  CreateRequestResult({
    required this.paymentRequest,
    required this.nostrWaitInfo,
  });
}

class FfiConverterCreateRequestResult {
  static CreateRequestResult lift(RustBuffer buf) {
    return FfiConverterCreateRequestResult.read(buf.asUint8List()).value;
  }

  static LiftRetVal<CreateRequestResult> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final paymentRequest_lifted = PaymentRequest.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final paymentRequest = paymentRequest_lifted.value;
    new_offset += paymentRequest_lifted.bytesRead;
    final nostrWaitInfo_lifted = FfiConverterOptionalNostrWaitInfo.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nostrWaitInfo = nostrWaitInfo_lifted.value;
    new_offset += nostrWaitInfo_lifted.bytesRead;
    return LiftRetVal(
      CreateRequestResult(
        paymentRequest: paymentRequest,
        nostrWaitInfo: nostrWaitInfo,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(CreateRequestResult value) {
    final total_length =
        PaymentRequest.allocationSize(value.paymentRequest) +
        FfiConverterOptionalNostrWaitInfo.allocationSize(value.nostrWaitInfo) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(CreateRequestResult value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += PaymentRequest.write(
      value.paymentRequest,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalNostrWaitInfo.write(
      value.nostrWaitInfo,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(CreateRequestResult value) {
    return PaymentRequest.allocationSize(value.paymentRequest) +
        FfiConverterOptionalNostrWaitInfo.allocationSize(value.nostrWaitInfo) +
        0;
  }
}

class DecodedInvoice {
  final PaymentType paymentType;
  final int? amountMsat;
  final int? expiry;
  final String? description;
  DecodedInvoice({
    required this.paymentType,
    required this.amountMsat,
    required this.expiry,
    required this.description,
  });
}

class FfiConverterDecodedInvoice {
  static DecodedInvoice lift(RustBuffer buf) {
    return FfiConverterDecodedInvoice.read(buf.asUint8List()).value;
  }

  static LiftRetVal<DecodedInvoice> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final paymentType_lifted = FfiConverterPaymentType.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final paymentType = paymentType_lifted.value;
    new_offset += paymentType_lifted.bytesRead;
    final amountMsat_lifted = FfiConverterOptionalUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amountMsat = amountMsat_lifted.value;
    new_offset += amountMsat_lifted.bytesRead;
    final expiry_lifted = FfiConverterOptionalUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final expiry = expiry_lifted.value;
    new_offset += expiry_lifted.bytesRead;
    final description_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final description = description_lifted.value;
    new_offset += description_lifted.bytesRead;
    return LiftRetVal(
      DecodedInvoice(
        paymentType: paymentType,
        amountMsat: amountMsat,
        expiry: expiry,
        description: description,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(DecodedInvoice value) {
    final total_length =
        FfiConverterPaymentType.allocationSize(value.paymentType) +
        FfiConverterOptionalUInt64.allocationSize(value.amountMsat) +
        FfiConverterOptionalUInt64.allocationSize(value.expiry) +
        FfiConverterOptionalString.allocationSize(value.description) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(DecodedInvoice value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterPaymentType.write(
      value.paymentType,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalUInt64.write(
      value.amountMsat,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalUInt64.write(
      value.expiry,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.description,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(DecodedInvoice value) {
    return FfiConverterPaymentType.allocationSize(value.paymentType) +
        FfiConverterOptionalUInt64.allocationSize(value.amountMsat) +
        FfiConverterOptionalUInt64.allocationSize(value.expiry) +
        FfiConverterOptionalString.allocationSize(value.description) +
        0;
  }
}

class FinalizedMelt {
  final String quoteId;
  final QuoteState state;
  final String? preimage;
  final List<Proof>? change;
  final Amount amount;
  final Amount feePaid;
  FinalizedMelt({
    required this.quoteId,
    required this.state,
    required this.preimage,
    required this.change,
    required this.amount,
    required this.feePaid,
  });
}

class FfiConverterFinalizedMelt {
  static FinalizedMelt lift(RustBuffer buf) {
    return FfiConverterFinalizedMelt.read(buf.asUint8List()).value;
  }

  static LiftRetVal<FinalizedMelt> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final quoteId_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final quoteId = quoteId_lifted.value;
    new_offset += quoteId_lifted.bytesRead;
    final state_lifted = FfiConverterQuoteState.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final state = state_lifted.value;
    new_offset += state_lifted.bytesRead;
    final preimage_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final preimage = preimage_lifted.value;
    new_offset += preimage_lifted.bytesRead;
    final change_lifted = FfiConverterOptionalSequenceProof.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final change = change_lifted.value;
    new_offset += change_lifted.bytesRead;
    final amount_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amount = amount_lifted.value;
    new_offset += amount_lifted.bytesRead;
    final feePaid_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final feePaid = feePaid_lifted.value;
    new_offset += feePaid_lifted.bytesRead;
    return LiftRetVal(
      FinalizedMelt(
        quoteId: quoteId,
        state: state,
        preimage: preimage,
        change: change,
        amount: amount,
        feePaid: feePaid,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(FinalizedMelt value) {
    final total_length =
        FfiConverterString.allocationSize(value.quoteId) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterOptionalString.allocationSize(value.preimage) +
        FfiConverterOptionalSequenceProof.allocationSize(value.change) +
        FfiConverterAmount.allocationSize(value.amount) +
        FfiConverterAmount.allocationSize(value.feePaid) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(FinalizedMelt value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.quoteId,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterQuoteState.write(
      value.state,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.preimage,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalSequenceProof.write(
      value.change,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.amount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.feePaid,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(FinalizedMelt value) {
    return FfiConverterString.allocationSize(value.quoteId) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterOptionalString.allocationSize(value.preimage) +
        FfiConverterOptionalSequenceProof.allocationSize(value.change) +
        FfiConverterAmount.allocationSize(value.amount) +
        FfiConverterAmount.allocationSize(value.feePaid) +
        0;
  }
}

class Id {
  final String hex;
  Id({required this.hex});
}

class FfiConverterId {
  static Id lift(RustBuffer buf) {
    return FfiConverterId.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Id> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final hex_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final hex = hex_lifted.value;
    new_offset += hex_lifted.bytesRead;
    return LiftRetVal(Id(hex: hex), new_offset - buf.offsetInBytes);
  }

  static RustBuffer lower(Id value) {
    final total_length = FfiConverterString.allocationSize(value.hex) + 0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(Id value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.hex,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(Id value) {
    return FfiConverterString.allocationSize(value.hex) + 0;
  }
}

class KeySet {
  final String id;
  final CurrencyUnit unit;
  final bool? active;
  final int inputFeePpk;
  final Map<int, String> keys;
  final int? finalExpiry;
  KeySet({
    required this.id,
    required this.unit,
    required this.active,
    required this.inputFeePpk,
    required this.keys,
    required this.finalExpiry,
  });
}

class FfiConverterKeySet {
  static KeySet lift(RustBuffer buf) {
    return FfiConverterKeySet.read(buf.asUint8List()).value;
  }

  static LiftRetVal<KeySet> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final id_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final id = id_lifted.value;
    new_offset += id_lifted.bytesRead;
    final unit_lifted = FfiConverterCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final active_lifted = FfiConverterOptionalBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final active = active_lifted.value;
    new_offset += active_lifted.bytesRead;
    final inputFeePpk_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final inputFeePpk = inputFeePpk_lifted.value;
    new_offset += inputFeePpk_lifted.bytesRead;
    final keys_lifted = FfiConverterMapUInt64ToString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final keys = keys_lifted.value;
    new_offset += keys_lifted.bytesRead;
    final finalExpiry_lifted = FfiConverterOptionalUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final finalExpiry = finalExpiry_lifted.value;
    new_offset += finalExpiry_lifted.bytesRead;
    return LiftRetVal(
      KeySet(
        id: id,
        unit: unit,
        active: active,
        inputFeePpk: inputFeePpk,
        keys: keys,
        finalExpiry: finalExpiry,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(KeySet value) {
    final total_length =
        FfiConverterString.allocationSize(value.id) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalBool.allocationSize(value.active) +
        FfiConverterUInt64.allocationSize(value.inputFeePpk) +
        FfiConverterMapUInt64ToString.allocationSize(value.keys) +
        FfiConverterOptionalUInt64.allocationSize(value.finalExpiry) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(KeySet value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.id,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalBool.write(
      value.active,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.inputFeePpk,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterMapUInt64ToString.write(
      value.keys,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalUInt64.write(
      value.finalExpiry,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(KeySet value) {
    return FfiConverterString.allocationSize(value.id) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalBool.allocationSize(value.active) +
        FfiConverterUInt64.allocationSize(value.inputFeePpk) +
        FfiConverterMapUInt64ToString.allocationSize(value.keys) +
        FfiConverterOptionalUInt64.allocationSize(value.finalExpiry) +
        0;
  }
}

class KeySetInfo {
  final String id;
  final CurrencyUnit unit;
  final bool active;
  final int inputFeePpk;
  KeySetInfo({
    required this.id,
    required this.unit,
    required this.active,
    required this.inputFeePpk,
  });
}

class FfiConverterKeySetInfo {
  static KeySetInfo lift(RustBuffer buf) {
    return FfiConverterKeySetInfo.read(buf.asUint8List()).value;
  }

  static LiftRetVal<KeySetInfo> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final id_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final id = id_lifted.value;
    new_offset += id_lifted.bytesRead;
    final unit_lifted = FfiConverterCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final active_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final active = active_lifted.value;
    new_offset += active_lifted.bytesRead;
    final inputFeePpk_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final inputFeePpk = inputFeePpk_lifted.value;
    new_offset += inputFeePpk_lifted.bytesRead;
    return LiftRetVal(
      KeySetInfo(id: id, unit: unit, active: active, inputFeePpk: inputFeePpk),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(KeySetInfo value) {
    final total_length =
        FfiConverterString.allocationSize(value.id) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterBool.allocationSize(value.active) +
        FfiConverterUInt64.allocationSize(value.inputFeePpk) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(KeySetInfo value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.id,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.active,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.inputFeePpk,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(KeySetInfo value) {
    return FfiConverterString.allocationSize(value.id) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterBool.allocationSize(value.active) +
        FfiConverterUInt64.allocationSize(value.inputFeePpk) +
        0;
  }
}

class Keys {
  final String id;
  final CurrencyUnit unit;
  final Map<int, String> keys;
  Keys({required this.id, required this.unit, required this.keys});
}

class FfiConverterKeys {
  static Keys lift(RustBuffer buf) {
    return FfiConverterKeys.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Keys> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final id_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final id = id_lifted.value;
    new_offset += id_lifted.bytesRead;
    final unit_lifted = FfiConverterCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final keys_lifted = FfiConverterMapUInt64ToString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final keys = keys_lifted.value;
    new_offset += keys_lifted.bytesRead;
    return LiftRetVal(
      Keys(id: id, unit: unit, keys: keys),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(Keys value) {
    final total_length =
        FfiConverterString.allocationSize(value.id) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterMapUInt64ToString.allocationSize(value.keys) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(Keys value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.id,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterMapUInt64ToString.write(
      value.keys,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(Keys value) {
    return FfiConverterString.allocationSize(value.id) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterMapUInt64ToString.allocationSize(value.keys) +
        0;
  }
}

class MeltConfirmOptions {
  final bool skipSwap;
  MeltConfirmOptions({required this.skipSwap});
}

class FfiConverterMeltConfirmOptions {
  static MeltConfirmOptions lift(RustBuffer buf) {
    return FfiConverterMeltConfirmOptions.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MeltConfirmOptions> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final skipSwap_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final skipSwap = skipSwap_lifted.value;
    new_offset += skipSwap_lifted.bytesRead;
    return LiftRetVal(
      MeltConfirmOptions(skipSwap: skipSwap),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(MeltConfirmOptions value) {
    final total_length = FfiConverterBool.allocationSize(value.skipSwap) + 0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(MeltConfirmOptions value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterBool.write(
      value.skipSwap,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(MeltConfirmOptions value) {
    return FfiConverterBool.allocationSize(value.skipSwap) + 0;
  }
}

class MeltMethodSettings {
  final PaymentMethod method;
  final CurrencyUnit unit;
  final Amount? minAmount;
  final Amount? maxAmount;
  final bool? amountless;
  MeltMethodSettings({
    required this.method,
    required this.unit,
    required this.minAmount,
    required this.maxAmount,
    required this.amountless,
  });
}

class FfiConverterMeltMethodSettings {
  static MeltMethodSettings lift(RustBuffer buf) {
    return FfiConverterMeltMethodSettings.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MeltMethodSettings> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final method_lifted = FfiConverterPaymentMethod.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final method = method_lifted.value;
    new_offset += method_lifted.bytesRead;
    final unit_lifted = FfiConverterCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final minAmount_lifted = FfiConverterOptionalAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final minAmount = minAmount_lifted.value;
    new_offset += minAmount_lifted.bytesRead;
    final maxAmount_lifted = FfiConverterOptionalAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final maxAmount = maxAmount_lifted.value;
    new_offset += maxAmount_lifted.bytesRead;
    final amountless_lifted = FfiConverterOptionalBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amountless = amountless_lifted.value;
    new_offset += amountless_lifted.bytesRead;
    return LiftRetVal(
      MeltMethodSettings(
        method: method,
        unit: unit,
        minAmount: minAmount,
        maxAmount: maxAmount,
        amountless: amountless,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(MeltMethodSettings value) {
    final total_length =
        FfiConverterPaymentMethod.allocationSize(value.method) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalAmount.allocationSize(value.minAmount) +
        FfiConverterOptionalAmount.allocationSize(value.maxAmount) +
        FfiConverterOptionalBool.allocationSize(value.amountless) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(MeltMethodSettings value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterPaymentMethod.write(
      value.method,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalAmount.write(
      value.minAmount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalAmount.write(
      value.maxAmount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalBool.write(
      value.amountless,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(MeltMethodSettings value) {
    return FfiConverterPaymentMethod.allocationSize(value.method) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalAmount.allocationSize(value.minAmount) +
        FfiConverterOptionalAmount.allocationSize(value.maxAmount) +
        FfiConverterOptionalBool.allocationSize(value.amountless) +
        0;
  }
}

class MeltQuote {
  final String id;
  final Amount amount;
  final CurrencyUnit unit;
  final String request;
  final Amount feeReserve;
  final QuoteState state;
  final int expiry;
  final String? paymentPreimage;
  final PaymentMethod paymentMethod;
  final String? usedByOperation;
  final int version;
  MeltQuote({
    required this.id,
    required this.amount,
    required this.unit,
    required this.request,
    required this.feeReserve,
    required this.state,
    required this.expiry,
    required this.paymentPreimage,
    required this.paymentMethod,
    required this.usedByOperation,
    required this.version,
  });
}

class FfiConverterMeltQuote {
  static MeltQuote lift(RustBuffer buf) {
    return FfiConverterMeltQuote.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MeltQuote> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final id_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final id = id_lifted.value;
    new_offset += id_lifted.bytesRead;
    final amount_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amount = amount_lifted.value;
    new_offset += amount_lifted.bytesRead;
    final unit_lifted = FfiConverterCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final request_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final request = request_lifted.value;
    new_offset += request_lifted.bytesRead;
    final feeReserve_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final feeReserve = feeReserve_lifted.value;
    new_offset += feeReserve_lifted.bytesRead;
    final state_lifted = FfiConverterQuoteState.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final state = state_lifted.value;
    new_offset += state_lifted.bytesRead;
    final expiry_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final expiry = expiry_lifted.value;
    new_offset += expiry_lifted.bytesRead;
    final paymentPreimage_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final paymentPreimage = paymentPreimage_lifted.value;
    new_offset += paymentPreimage_lifted.bytesRead;
    final paymentMethod_lifted = FfiConverterPaymentMethod.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final paymentMethod = paymentMethod_lifted.value;
    new_offset += paymentMethod_lifted.bytesRead;
    final usedByOperation_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final usedByOperation = usedByOperation_lifted.value;
    new_offset += usedByOperation_lifted.bytesRead;
    final version_lifted = FfiConverterUInt32.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final version = version_lifted.value;
    new_offset += version_lifted.bytesRead;
    return LiftRetVal(
      MeltQuote(
        id: id,
        amount: amount,
        unit: unit,
        request: request,
        feeReserve: feeReserve,
        state: state,
        expiry: expiry,
        paymentPreimage: paymentPreimage,
        paymentMethod: paymentMethod,
        usedByOperation: usedByOperation,
        version: version,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(MeltQuote value) {
    final total_length =
        FfiConverterString.allocationSize(value.id) +
        FfiConverterAmount.allocationSize(value.amount) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterString.allocationSize(value.request) +
        FfiConverterAmount.allocationSize(value.feeReserve) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterUInt64.allocationSize(value.expiry) +
        FfiConverterOptionalString.allocationSize(value.paymentPreimage) +
        FfiConverterPaymentMethod.allocationSize(value.paymentMethod) +
        FfiConverterOptionalString.allocationSize(value.usedByOperation) +
        FfiConverterUInt32.allocationSize(value.version) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(MeltQuote value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.id,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.amount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.request,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.feeReserve,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterQuoteState.write(
      value.state,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.expiry,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.paymentPreimage,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterPaymentMethod.write(
      value.paymentMethod,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.usedByOperation,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt32.write(
      value.version,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(MeltQuote value) {
    return FfiConverterString.allocationSize(value.id) +
        FfiConverterAmount.allocationSize(value.amount) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterString.allocationSize(value.request) +
        FfiConverterAmount.allocationSize(value.feeReserve) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterUInt64.allocationSize(value.expiry) +
        FfiConverterOptionalString.allocationSize(value.paymentPreimage) +
        FfiConverterPaymentMethod.allocationSize(value.paymentMethod) +
        FfiConverterOptionalString.allocationSize(value.usedByOperation) +
        FfiConverterUInt32.allocationSize(value.version) +
        0;
  }
}

class MeltQuoteBolt11Response {
  final String quote;
  final Amount amount;
  final Amount feeReserve;
  final QuoteState state;
  final int expiry;
  final String? paymentPreimage;
  final String? request;
  final CurrencyUnit? unit;
  MeltQuoteBolt11Response({
    required this.quote,
    required this.amount,
    required this.feeReserve,
    required this.state,
    required this.expiry,
    required this.paymentPreimage,
    required this.request,
    required this.unit,
  });
}

class FfiConverterMeltQuoteBolt11Response {
  static MeltQuoteBolt11Response lift(RustBuffer buf) {
    return FfiConverterMeltQuoteBolt11Response.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MeltQuoteBolt11Response> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final quote_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final quote = quote_lifted.value;
    new_offset += quote_lifted.bytesRead;
    final amount_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amount = amount_lifted.value;
    new_offset += amount_lifted.bytesRead;
    final feeReserve_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final feeReserve = feeReserve_lifted.value;
    new_offset += feeReserve_lifted.bytesRead;
    final state_lifted = FfiConverterQuoteState.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final state = state_lifted.value;
    new_offset += state_lifted.bytesRead;
    final expiry_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final expiry = expiry_lifted.value;
    new_offset += expiry_lifted.bytesRead;
    final paymentPreimage_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final paymentPreimage = paymentPreimage_lifted.value;
    new_offset += paymentPreimage_lifted.bytesRead;
    final request_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final request = request_lifted.value;
    new_offset += request_lifted.bytesRead;
    final unit_lifted = FfiConverterOptionalCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    return LiftRetVal(
      MeltQuoteBolt11Response(
        quote: quote,
        amount: amount,
        feeReserve: feeReserve,
        state: state,
        expiry: expiry,
        paymentPreimage: paymentPreimage,
        request: request,
        unit: unit,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(MeltQuoteBolt11Response value) {
    final total_length =
        FfiConverterString.allocationSize(value.quote) +
        FfiConverterAmount.allocationSize(value.amount) +
        FfiConverterAmount.allocationSize(value.feeReserve) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterUInt64.allocationSize(value.expiry) +
        FfiConverterOptionalString.allocationSize(value.paymentPreimage) +
        FfiConverterOptionalString.allocationSize(value.request) +
        FfiConverterOptionalCurrencyUnit.allocationSize(value.unit) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(MeltQuoteBolt11Response value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.quote,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.amount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.feeReserve,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterQuoteState.write(
      value.state,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.expiry,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.paymentPreimage,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.request,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(MeltQuoteBolt11Response value) {
    return FfiConverterString.allocationSize(value.quote) +
        FfiConverterAmount.allocationSize(value.amount) +
        FfiConverterAmount.allocationSize(value.feeReserve) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterUInt64.allocationSize(value.expiry) +
        FfiConverterOptionalString.allocationSize(value.paymentPreimage) +
        FfiConverterOptionalString.allocationSize(value.request) +
        FfiConverterOptionalCurrencyUnit.allocationSize(value.unit) +
        0;
  }
}

class MeltQuoteCustomResponse {
  final String quote;
  final Amount amount;
  final Amount feeReserve;
  final QuoteState state;
  final int expiry;
  final String? paymentPreimage;
  final String? request;
  final CurrencyUnit? unit;
  final String? extra;
  MeltQuoteCustomResponse({
    required this.quote,
    required this.amount,
    required this.feeReserve,
    required this.state,
    required this.expiry,
    required this.paymentPreimage,
    required this.request,
    required this.unit,
    required this.extra,
  });
}

class FfiConverterMeltQuoteCustomResponse {
  static MeltQuoteCustomResponse lift(RustBuffer buf) {
    return FfiConverterMeltQuoteCustomResponse.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MeltQuoteCustomResponse> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final quote_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final quote = quote_lifted.value;
    new_offset += quote_lifted.bytesRead;
    final amount_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amount = amount_lifted.value;
    new_offset += amount_lifted.bytesRead;
    final feeReserve_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final feeReserve = feeReserve_lifted.value;
    new_offset += feeReserve_lifted.bytesRead;
    final state_lifted = FfiConverterQuoteState.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final state = state_lifted.value;
    new_offset += state_lifted.bytesRead;
    final expiry_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final expiry = expiry_lifted.value;
    new_offset += expiry_lifted.bytesRead;
    final paymentPreimage_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final paymentPreimage = paymentPreimage_lifted.value;
    new_offset += paymentPreimage_lifted.bytesRead;
    final request_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final request = request_lifted.value;
    new_offset += request_lifted.bytesRead;
    final unit_lifted = FfiConverterOptionalCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final extra_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final extra = extra_lifted.value;
    new_offset += extra_lifted.bytesRead;
    return LiftRetVal(
      MeltQuoteCustomResponse(
        quote: quote,
        amount: amount,
        feeReserve: feeReserve,
        state: state,
        expiry: expiry,
        paymentPreimage: paymentPreimage,
        request: request,
        unit: unit,
        extra: extra,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(MeltQuoteCustomResponse value) {
    final total_length =
        FfiConverterString.allocationSize(value.quote) +
        FfiConverterAmount.allocationSize(value.amount) +
        FfiConverterAmount.allocationSize(value.feeReserve) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterUInt64.allocationSize(value.expiry) +
        FfiConverterOptionalString.allocationSize(value.paymentPreimage) +
        FfiConverterOptionalString.allocationSize(value.request) +
        FfiConverterOptionalCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalString.allocationSize(value.extra) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(MeltQuoteCustomResponse value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.quote,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.amount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.feeReserve,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterQuoteState.write(
      value.state,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.expiry,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.paymentPreimage,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.request,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.extra,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(MeltQuoteCustomResponse value) {
    return FfiConverterString.allocationSize(value.quote) +
        FfiConverterAmount.allocationSize(value.amount) +
        FfiConverterAmount.allocationSize(value.feeReserve) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterUInt64.allocationSize(value.expiry) +
        FfiConverterOptionalString.allocationSize(value.paymentPreimage) +
        FfiConverterOptionalString.allocationSize(value.request) +
        FfiConverterOptionalCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalString.allocationSize(value.extra) +
        0;
  }
}

class MintBackup {
  final List<MintUrl> mints;
  final int timestamp;
  MintBackup({required this.mints, required this.timestamp});
}

class FfiConverterMintBackup {
  static MintBackup lift(RustBuffer buf) {
    return FfiConverterMintBackup.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MintBackup> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final mints_lifted = FfiConverterSequenceMintUrl.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final mints = mints_lifted.value;
    new_offset += mints_lifted.bytesRead;
    final timestamp_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final timestamp = timestamp_lifted.value;
    new_offset += timestamp_lifted.bytesRead;
    return LiftRetVal(
      MintBackup(mints: mints, timestamp: timestamp),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(MintBackup value) {
    final total_length =
        FfiConverterSequenceMintUrl.allocationSize(value.mints) +
        FfiConverterUInt64.allocationSize(value.timestamp) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(MintBackup value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterSequenceMintUrl.write(
      value.mints,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.timestamp,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(MintBackup value) {
    return FfiConverterSequenceMintUrl.allocationSize(value.mints) +
        FfiConverterUInt64.allocationSize(value.timestamp) +
        0;
  }
}

class MintInfo {
  final String? name;
  final String? pubkey;
  final MintVersion? version;
  final String? description;
  final String? descriptionLong;
  final List<ContactInfo>? contact;
  final Nuts nuts;
  final String? iconUrl;
  final List<String>? urls;
  final String? motd;
  final int? time;
  final String? tosUrl;
  MintInfo({
    required this.name,
    required this.pubkey,
    required this.version,
    required this.description,
    required this.descriptionLong,
    required this.contact,
    required this.nuts,
    required this.iconUrl,
    required this.urls,
    required this.motd,
    required this.time,
    required this.tosUrl,
  });
}

class FfiConverterMintInfo {
  static MintInfo lift(RustBuffer buf) {
    return FfiConverterMintInfo.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MintInfo> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final name_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final name = name_lifted.value;
    new_offset += name_lifted.bytesRead;
    final pubkey_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final pubkey = pubkey_lifted.value;
    new_offset += pubkey_lifted.bytesRead;
    final version_lifted = FfiConverterOptionalMintVersion.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final version = version_lifted.value;
    new_offset += version_lifted.bytesRead;
    final description_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final description = description_lifted.value;
    new_offset += description_lifted.bytesRead;
    final descriptionLong_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final descriptionLong = descriptionLong_lifted.value;
    new_offset += descriptionLong_lifted.bytesRead;
    final contact_lifted = FfiConverterOptionalSequenceContactInfo.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final contact = contact_lifted.value;
    new_offset += contact_lifted.bytesRead;
    final nuts_lifted = FfiConverterNuts.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nuts = nuts_lifted.value;
    new_offset += nuts_lifted.bytesRead;
    final iconUrl_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final iconUrl = iconUrl_lifted.value;
    new_offset += iconUrl_lifted.bytesRead;
    final urls_lifted = FfiConverterOptionalSequenceString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final urls = urls_lifted.value;
    new_offset += urls_lifted.bytesRead;
    final motd_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final motd = motd_lifted.value;
    new_offset += motd_lifted.bytesRead;
    final time_lifted = FfiConverterOptionalUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final time = time_lifted.value;
    new_offset += time_lifted.bytesRead;
    final tosUrl_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final tosUrl = tosUrl_lifted.value;
    new_offset += tosUrl_lifted.bytesRead;
    return LiftRetVal(
      MintInfo(
        name: name,
        pubkey: pubkey,
        version: version,
        description: description,
        descriptionLong: descriptionLong,
        contact: contact,
        nuts: nuts,
        iconUrl: iconUrl,
        urls: urls,
        motd: motd,
        time: time,
        tosUrl: tosUrl,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(MintInfo value) {
    final total_length =
        FfiConverterOptionalString.allocationSize(value.name) +
        FfiConverterOptionalString.allocationSize(value.pubkey) +
        FfiConverterOptionalMintVersion.allocationSize(value.version) +
        FfiConverterOptionalString.allocationSize(value.description) +
        FfiConverterOptionalString.allocationSize(value.descriptionLong) +
        FfiConverterOptionalSequenceContactInfo.allocationSize(value.contact) +
        FfiConverterNuts.allocationSize(value.nuts) +
        FfiConverterOptionalString.allocationSize(value.iconUrl) +
        FfiConverterOptionalSequenceString.allocationSize(value.urls) +
        FfiConverterOptionalString.allocationSize(value.motd) +
        FfiConverterOptionalUInt64.allocationSize(value.time) +
        FfiConverterOptionalString.allocationSize(value.tosUrl) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(MintInfo value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterOptionalString.write(
      value.name,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.pubkey,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalMintVersion.write(
      value.version,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.description,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.descriptionLong,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalSequenceContactInfo.write(
      value.contact,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterNuts.write(
      value.nuts,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.iconUrl,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalSequenceString.write(
      value.urls,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.motd,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalUInt64.write(
      value.time,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.tosUrl,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(MintInfo value) {
    return FfiConverterOptionalString.allocationSize(value.name) +
        FfiConverterOptionalString.allocationSize(value.pubkey) +
        FfiConverterOptionalMintVersion.allocationSize(value.version) +
        FfiConverterOptionalString.allocationSize(value.description) +
        FfiConverterOptionalString.allocationSize(value.descriptionLong) +
        FfiConverterOptionalSequenceContactInfo.allocationSize(value.contact) +
        FfiConverterNuts.allocationSize(value.nuts) +
        FfiConverterOptionalString.allocationSize(value.iconUrl) +
        FfiConverterOptionalSequenceString.allocationSize(value.urls) +
        FfiConverterOptionalString.allocationSize(value.motd) +
        FfiConverterOptionalUInt64.allocationSize(value.time) +
        FfiConverterOptionalString.allocationSize(value.tosUrl) +
        0;
  }
}

class MintMethodSettings {
  final PaymentMethod method;
  final CurrencyUnit unit;
  final Amount? minAmount;
  final Amount? maxAmount;
  final bool? description;
  MintMethodSettings({
    required this.method,
    required this.unit,
    required this.minAmount,
    required this.maxAmount,
    required this.description,
  });
}

class FfiConverterMintMethodSettings {
  static MintMethodSettings lift(RustBuffer buf) {
    return FfiConverterMintMethodSettings.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MintMethodSettings> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final method_lifted = FfiConverterPaymentMethod.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final method = method_lifted.value;
    new_offset += method_lifted.bytesRead;
    final unit_lifted = FfiConverterCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final minAmount_lifted = FfiConverterOptionalAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final minAmount = minAmount_lifted.value;
    new_offset += minAmount_lifted.bytesRead;
    final maxAmount_lifted = FfiConverterOptionalAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final maxAmount = maxAmount_lifted.value;
    new_offset += maxAmount_lifted.bytesRead;
    final description_lifted = FfiConverterOptionalBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final description = description_lifted.value;
    new_offset += description_lifted.bytesRead;
    return LiftRetVal(
      MintMethodSettings(
        method: method,
        unit: unit,
        minAmount: minAmount,
        maxAmount: maxAmount,
        description: description,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(MintMethodSettings value) {
    final total_length =
        FfiConverterPaymentMethod.allocationSize(value.method) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalAmount.allocationSize(value.minAmount) +
        FfiConverterOptionalAmount.allocationSize(value.maxAmount) +
        FfiConverterOptionalBool.allocationSize(value.description) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(MintMethodSettings value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterPaymentMethod.write(
      value.method,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalAmount.write(
      value.minAmount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalAmount.write(
      value.maxAmount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalBool.write(
      value.description,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(MintMethodSettings value) {
    return FfiConverterPaymentMethod.allocationSize(value.method) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalAmount.allocationSize(value.minAmount) +
        FfiConverterOptionalAmount.allocationSize(value.maxAmount) +
        FfiConverterOptionalBool.allocationSize(value.description) +
        0;
  }
}

class MintQuote {
  final String id;
  final Amount? amount;
  final CurrencyUnit unit;
  final String request;
  final QuoteState state;
  final int expiry;
  final MintUrl mintUrl;
  final Amount amountIssued;
  final Amount amountPaid;
  final PaymentMethod paymentMethod;
  final String? secretKey;
  final String? usedByOperation;
  final int version;
  MintQuote({
    required this.id,
    required this.amount,
    required this.unit,
    required this.request,
    required this.state,
    required this.expiry,
    required this.mintUrl,
    required this.amountIssued,
    required this.amountPaid,
    required this.paymentMethod,
    required this.secretKey,
    required this.usedByOperation,
    required this.version,
  });
}

class FfiConverterMintQuote {
  static MintQuote lift(RustBuffer buf) {
    return FfiConverterMintQuote.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MintQuote> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final id_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final id = id_lifted.value;
    new_offset += id_lifted.bytesRead;
    final amount_lifted = FfiConverterOptionalAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amount = amount_lifted.value;
    new_offset += amount_lifted.bytesRead;
    final unit_lifted = FfiConverterCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final request_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final request = request_lifted.value;
    new_offset += request_lifted.bytesRead;
    final state_lifted = FfiConverterQuoteState.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final state = state_lifted.value;
    new_offset += state_lifted.bytesRead;
    final expiry_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final expiry = expiry_lifted.value;
    new_offset += expiry_lifted.bytesRead;
    final mintUrl_lifted = FfiConverterMintUrl.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final mintUrl = mintUrl_lifted.value;
    new_offset += mintUrl_lifted.bytesRead;
    final amountIssued_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amountIssued = amountIssued_lifted.value;
    new_offset += amountIssued_lifted.bytesRead;
    final amountPaid_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amountPaid = amountPaid_lifted.value;
    new_offset += amountPaid_lifted.bytesRead;
    final paymentMethod_lifted = FfiConverterPaymentMethod.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final paymentMethod = paymentMethod_lifted.value;
    new_offset += paymentMethod_lifted.bytesRead;
    final secretKey_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final secretKey = secretKey_lifted.value;
    new_offset += secretKey_lifted.bytesRead;
    final usedByOperation_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final usedByOperation = usedByOperation_lifted.value;
    new_offset += usedByOperation_lifted.bytesRead;
    final version_lifted = FfiConverterUInt32.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final version = version_lifted.value;
    new_offset += version_lifted.bytesRead;
    return LiftRetVal(
      MintQuote(
        id: id,
        amount: amount,
        unit: unit,
        request: request,
        state: state,
        expiry: expiry,
        mintUrl: mintUrl,
        amountIssued: amountIssued,
        amountPaid: amountPaid,
        paymentMethod: paymentMethod,
        secretKey: secretKey,
        usedByOperation: usedByOperation,
        version: version,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(MintQuote value) {
    final total_length =
        FfiConverterString.allocationSize(value.id) +
        FfiConverterOptionalAmount.allocationSize(value.amount) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterString.allocationSize(value.request) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterUInt64.allocationSize(value.expiry) +
        FfiConverterMintUrl.allocationSize(value.mintUrl) +
        FfiConverterAmount.allocationSize(value.amountIssued) +
        FfiConverterAmount.allocationSize(value.amountPaid) +
        FfiConverterPaymentMethod.allocationSize(value.paymentMethod) +
        FfiConverterOptionalString.allocationSize(value.secretKey) +
        FfiConverterOptionalString.allocationSize(value.usedByOperation) +
        FfiConverterUInt32.allocationSize(value.version) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(MintQuote value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.id,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalAmount.write(
      value.amount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.request,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterQuoteState.write(
      value.state,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.expiry,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterMintUrl.write(
      value.mintUrl,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.amountIssued,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.amountPaid,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterPaymentMethod.write(
      value.paymentMethod,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.secretKey,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.usedByOperation,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt32.write(
      value.version,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(MintQuote value) {
    return FfiConverterString.allocationSize(value.id) +
        FfiConverterOptionalAmount.allocationSize(value.amount) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterString.allocationSize(value.request) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterUInt64.allocationSize(value.expiry) +
        FfiConverterMintUrl.allocationSize(value.mintUrl) +
        FfiConverterAmount.allocationSize(value.amountIssued) +
        FfiConverterAmount.allocationSize(value.amountPaid) +
        FfiConverterPaymentMethod.allocationSize(value.paymentMethod) +
        FfiConverterOptionalString.allocationSize(value.secretKey) +
        FfiConverterOptionalString.allocationSize(value.usedByOperation) +
        FfiConverterUInt32.allocationSize(value.version) +
        0;
  }
}

class MintQuoteBolt11Response {
  final String quote;
  final String request;
  final QuoteState state;
  final int? expiry;
  final Amount? amount;
  final CurrencyUnit? unit;
  final String? pubkey;
  MintQuoteBolt11Response({
    required this.quote,
    required this.request,
    required this.state,
    required this.expiry,
    required this.amount,
    required this.unit,
    required this.pubkey,
  });
}

class FfiConverterMintQuoteBolt11Response {
  static MintQuoteBolt11Response lift(RustBuffer buf) {
    return FfiConverterMintQuoteBolt11Response.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MintQuoteBolt11Response> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final quote_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final quote = quote_lifted.value;
    new_offset += quote_lifted.bytesRead;
    final request_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final request = request_lifted.value;
    new_offset += request_lifted.bytesRead;
    final state_lifted = FfiConverterQuoteState.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final state = state_lifted.value;
    new_offset += state_lifted.bytesRead;
    final expiry_lifted = FfiConverterOptionalUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final expiry = expiry_lifted.value;
    new_offset += expiry_lifted.bytesRead;
    final amount_lifted = FfiConverterOptionalAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amount = amount_lifted.value;
    new_offset += amount_lifted.bytesRead;
    final unit_lifted = FfiConverterOptionalCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final pubkey_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final pubkey = pubkey_lifted.value;
    new_offset += pubkey_lifted.bytesRead;
    return LiftRetVal(
      MintQuoteBolt11Response(
        quote: quote,
        request: request,
        state: state,
        expiry: expiry,
        amount: amount,
        unit: unit,
        pubkey: pubkey,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(MintQuoteBolt11Response value) {
    final total_length =
        FfiConverterString.allocationSize(value.quote) +
        FfiConverterString.allocationSize(value.request) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterOptionalUInt64.allocationSize(value.expiry) +
        FfiConverterOptionalAmount.allocationSize(value.amount) +
        FfiConverterOptionalCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalString.allocationSize(value.pubkey) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(MintQuoteBolt11Response value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.quote,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.request,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterQuoteState.write(
      value.state,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalUInt64.write(
      value.expiry,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalAmount.write(
      value.amount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.pubkey,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(MintQuoteBolt11Response value) {
    return FfiConverterString.allocationSize(value.quote) +
        FfiConverterString.allocationSize(value.request) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterOptionalUInt64.allocationSize(value.expiry) +
        FfiConverterOptionalAmount.allocationSize(value.amount) +
        FfiConverterOptionalCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalString.allocationSize(value.pubkey) +
        0;
  }
}

class MintQuoteCustomResponse {
  final String quote;
  final String request;
  final QuoteState state;
  final int? expiry;
  final Amount? amount;
  final CurrencyUnit? unit;
  final String? pubkey;
  final String? extra;
  MintQuoteCustomResponse({
    required this.quote,
    required this.request,
    required this.state,
    required this.expiry,
    required this.amount,
    required this.unit,
    required this.pubkey,
    required this.extra,
  });
}

class FfiConverterMintQuoteCustomResponse {
  static MintQuoteCustomResponse lift(RustBuffer buf) {
    return FfiConverterMintQuoteCustomResponse.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MintQuoteCustomResponse> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final quote_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final quote = quote_lifted.value;
    new_offset += quote_lifted.bytesRead;
    final request_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final request = request_lifted.value;
    new_offset += request_lifted.bytesRead;
    final state_lifted = FfiConverterQuoteState.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final state = state_lifted.value;
    new_offset += state_lifted.bytesRead;
    final expiry_lifted = FfiConverterOptionalUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final expiry = expiry_lifted.value;
    new_offset += expiry_lifted.bytesRead;
    final amount_lifted = FfiConverterOptionalAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amount = amount_lifted.value;
    new_offset += amount_lifted.bytesRead;
    final unit_lifted = FfiConverterOptionalCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final pubkey_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final pubkey = pubkey_lifted.value;
    new_offset += pubkey_lifted.bytesRead;
    final extra_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final extra = extra_lifted.value;
    new_offset += extra_lifted.bytesRead;
    return LiftRetVal(
      MintQuoteCustomResponse(
        quote: quote,
        request: request,
        state: state,
        expiry: expiry,
        amount: amount,
        unit: unit,
        pubkey: pubkey,
        extra: extra,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(MintQuoteCustomResponse value) {
    final total_length =
        FfiConverterString.allocationSize(value.quote) +
        FfiConverterString.allocationSize(value.request) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterOptionalUInt64.allocationSize(value.expiry) +
        FfiConverterOptionalAmount.allocationSize(value.amount) +
        FfiConverterOptionalCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalString.allocationSize(value.pubkey) +
        FfiConverterOptionalString.allocationSize(value.extra) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(MintQuoteCustomResponse value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.quote,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.request,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterQuoteState.write(
      value.state,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalUInt64.write(
      value.expiry,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalAmount.write(
      value.amount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.pubkey,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.extra,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(MintQuoteCustomResponse value) {
    return FfiConverterString.allocationSize(value.quote) +
        FfiConverterString.allocationSize(value.request) +
        FfiConverterQuoteState.allocationSize(value.state) +
        FfiConverterOptionalUInt64.allocationSize(value.expiry) +
        FfiConverterOptionalAmount.allocationSize(value.amount) +
        FfiConverterOptionalCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalString.allocationSize(value.pubkey) +
        FfiConverterOptionalString.allocationSize(value.extra) +
        0;
  }
}

class MintUrl {
  final String url;
  MintUrl({required this.url});
}

class FfiConverterMintUrl {
  static MintUrl lift(RustBuffer buf) {
    return FfiConverterMintUrl.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MintUrl> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final url_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final url = url_lifted.value;
    new_offset += url_lifted.bytesRead;
    return LiftRetVal(MintUrl(url: url), new_offset - buf.offsetInBytes);
  }

  static RustBuffer lower(MintUrl value) {
    final total_length = FfiConverterString.allocationSize(value.url) + 0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(MintUrl value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.url,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(MintUrl value) {
    return FfiConverterString.allocationSize(value.url) + 0;
  }
}

class MintVersion {
  final String name;
  final String version;
  MintVersion({required this.name, required this.version});
}

class FfiConverterMintVersion {
  static MintVersion lift(RustBuffer buf) {
    return FfiConverterMintVersion.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MintVersion> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final name_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final name = name_lifted.value;
    new_offset += name_lifted.bytesRead;
    final version_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final version = version_lifted.value;
    new_offset += version_lifted.bytesRead;
    return LiftRetVal(
      MintVersion(name: name, version: version),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(MintVersion value) {
    final total_length =
        FfiConverterString.allocationSize(value.name) +
        FfiConverterString.allocationSize(value.version) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(MintVersion value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.name,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.version,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(MintVersion value) {
    return FfiConverterString.allocationSize(value.name) +
        FfiConverterString.allocationSize(value.version) +
        0;
  }
}

class NpubCashQuote {
  final String id;
  final int amount;
  final String unit;
  final int createdAt;
  final int? paidAt;
  final int? expiresAt;
  final String? mintUrl;
  final String? request;
  final String? state;
  final bool? locked;
  NpubCashQuote({
    required this.id,
    required this.amount,
    required this.unit,
    required this.createdAt,
    required this.paidAt,
    required this.expiresAt,
    required this.mintUrl,
    required this.request,
    required this.state,
    required this.locked,
  });
}

class FfiConverterNpubCashQuote {
  static NpubCashQuote lift(RustBuffer buf) {
    return FfiConverterNpubCashQuote.read(buf.asUint8List()).value;
  }

  static LiftRetVal<NpubCashQuote> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final id_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final id = id_lifted.value;
    new_offset += id_lifted.bytesRead;
    final amount_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amount = amount_lifted.value;
    new_offset += amount_lifted.bytesRead;
    final unit_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final createdAt_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final createdAt = createdAt_lifted.value;
    new_offset += createdAt_lifted.bytesRead;
    final paidAt_lifted = FfiConverterOptionalUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final paidAt = paidAt_lifted.value;
    new_offset += paidAt_lifted.bytesRead;
    final expiresAt_lifted = FfiConverterOptionalUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final expiresAt = expiresAt_lifted.value;
    new_offset += expiresAt_lifted.bytesRead;
    final mintUrl_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final mintUrl = mintUrl_lifted.value;
    new_offset += mintUrl_lifted.bytesRead;
    final request_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final request = request_lifted.value;
    new_offset += request_lifted.bytesRead;
    final state_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final state = state_lifted.value;
    new_offset += state_lifted.bytesRead;
    final locked_lifted = FfiConverterOptionalBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final locked = locked_lifted.value;
    new_offset += locked_lifted.bytesRead;
    return LiftRetVal(
      NpubCashQuote(
        id: id,
        amount: amount,
        unit: unit,
        createdAt: createdAt,
        paidAt: paidAt,
        expiresAt: expiresAt,
        mintUrl: mintUrl,
        request: request,
        state: state,
        locked: locked,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(NpubCashQuote value) {
    final total_length =
        FfiConverterString.allocationSize(value.id) +
        FfiConverterUInt64.allocationSize(value.amount) +
        FfiConverterString.allocationSize(value.unit) +
        FfiConverterUInt64.allocationSize(value.createdAt) +
        FfiConverterOptionalUInt64.allocationSize(value.paidAt) +
        FfiConverterOptionalUInt64.allocationSize(value.expiresAt) +
        FfiConverterOptionalString.allocationSize(value.mintUrl) +
        FfiConverterOptionalString.allocationSize(value.request) +
        FfiConverterOptionalString.allocationSize(value.state) +
        FfiConverterOptionalBool.allocationSize(value.locked) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(NpubCashQuote value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.id,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.amount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.createdAt,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalUInt64.write(
      value.paidAt,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalUInt64.write(
      value.expiresAt,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.mintUrl,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.request,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.state,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalBool.write(
      value.locked,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(NpubCashQuote value) {
    return FfiConverterString.allocationSize(value.id) +
        FfiConverterUInt64.allocationSize(value.amount) +
        FfiConverterString.allocationSize(value.unit) +
        FfiConverterUInt64.allocationSize(value.createdAt) +
        FfiConverterOptionalUInt64.allocationSize(value.paidAt) +
        FfiConverterOptionalUInt64.allocationSize(value.expiresAt) +
        FfiConverterOptionalString.allocationSize(value.mintUrl) +
        FfiConverterOptionalString.allocationSize(value.request) +
        FfiConverterOptionalString.allocationSize(value.state) +
        FfiConverterOptionalBool.allocationSize(value.locked) +
        0;
  }
}

class NpubCashUserResponse {
  final bool error;
  final String pubkey;
  final String? mintUrl;
  final bool lockQuote;
  NpubCashUserResponse({
    required this.error,
    required this.pubkey,
    required this.mintUrl,
    required this.lockQuote,
  });
}

class FfiConverterNpubCashUserResponse {
  static NpubCashUserResponse lift(RustBuffer buf) {
    return FfiConverterNpubCashUserResponse.read(buf.asUint8List()).value;
  }

  static LiftRetVal<NpubCashUserResponse> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final error_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final error = error_lifted.value;
    new_offset += error_lifted.bytesRead;
    final pubkey_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final pubkey = pubkey_lifted.value;
    new_offset += pubkey_lifted.bytesRead;
    final mintUrl_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final mintUrl = mintUrl_lifted.value;
    new_offset += mintUrl_lifted.bytesRead;
    final lockQuote_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final lockQuote = lockQuote_lifted.value;
    new_offset += lockQuote_lifted.bytesRead;
    return LiftRetVal(
      NpubCashUserResponse(
        error: error,
        pubkey: pubkey,
        mintUrl: mintUrl,
        lockQuote: lockQuote,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(NpubCashUserResponse value) {
    final total_length =
        FfiConverterBool.allocationSize(value.error) +
        FfiConverterString.allocationSize(value.pubkey) +
        FfiConverterOptionalString.allocationSize(value.mintUrl) +
        FfiConverterBool.allocationSize(value.lockQuote) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(NpubCashUserResponse value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterBool.write(
      value.error,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.pubkey,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.mintUrl,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.lockQuote,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(NpubCashUserResponse value) {
    return FfiConverterBool.allocationSize(value.error) +
        FfiConverterString.allocationSize(value.pubkey) +
        FfiConverterOptionalString.allocationSize(value.mintUrl) +
        FfiConverterBool.allocationSize(value.lockQuote) +
        0;
  }
}

class Nut04Settings {
  final List<MintMethodSettings> methods;
  final bool disabled;
  Nut04Settings({required this.methods, required this.disabled});
}

class FfiConverterNut04Settings {
  static Nut04Settings lift(RustBuffer buf) {
    return FfiConverterNut04Settings.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Nut04Settings> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final methods_lifted = FfiConverterSequenceMintMethodSettings.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final methods = methods_lifted.value;
    new_offset += methods_lifted.bytesRead;
    final disabled_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final disabled = disabled_lifted.value;
    new_offset += disabled_lifted.bytesRead;
    return LiftRetVal(
      Nut04Settings(methods: methods, disabled: disabled),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(Nut04Settings value) {
    final total_length =
        FfiConverterSequenceMintMethodSettings.allocationSize(value.methods) +
        FfiConverterBool.allocationSize(value.disabled) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(Nut04Settings value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterSequenceMintMethodSettings.write(
      value.methods,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.disabled,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(Nut04Settings value) {
    return FfiConverterSequenceMintMethodSettings.allocationSize(
          value.methods,
        ) +
        FfiConverterBool.allocationSize(value.disabled) +
        0;
  }
}

class Nut05Settings {
  final List<MeltMethodSettings> methods;
  final bool disabled;
  Nut05Settings({required this.methods, required this.disabled});
}

class FfiConverterNut05Settings {
  static Nut05Settings lift(RustBuffer buf) {
    return FfiConverterNut05Settings.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Nut05Settings> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final methods_lifted = FfiConverterSequenceMeltMethodSettings.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final methods = methods_lifted.value;
    new_offset += methods_lifted.bytesRead;
    final disabled_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final disabled = disabled_lifted.value;
    new_offset += disabled_lifted.bytesRead;
    return LiftRetVal(
      Nut05Settings(methods: methods, disabled: disabled),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(Nut05Settings value) {
    final total_length =
        FfiConverterSequenceMeltMethodSettings.allocationSize(value.methods) +
        FfiConverterBool.allocationSize(value.disabled) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(Nut05Settings value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterSequenceMeltMethodSettings.write(
      value.methods,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.disabled,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(Nut05Settings value) {
    return FfiConverterSequenceMeltMethodSettings.allocationSize(
          value.methods,
        ) +
        FfiConverterBool.allocationSize(value.disabled) +
        0;
  }
}

class Nuts {
  final Nut04Settings nut04;
  final Nut05Settings nut05;
  final bool nut07Supported;
  final bool nut08Supported;
  final bool nut09Supported;
  final bool nut10Supported;
  final bool nut11Supported;
  final bool nut12Supported;
  final bool nut14Supported;
  final bool nut20Supported;
  final ClearAuthSettings? nut21;
  final BlindAuthSettings? nut22;
  final List<CurrencyUnit> mintUnits;
  final List<CurrencyUnit> meltUnits;
  Nuts({
    required this.nut04,
    required this.nut05,
    required this.nut07Supported,
    required this.nut08Supported,
    required this.nut09Supported,
    required this.nut10Supported,
    required this.nut11Supported,
    required this.nut12Supported,
    required this.nut14Supported,
    required this.nut20Supported,
    required this.nut21,
    required this.nut22,
    required this.mintUnits,
    required this.meltUnits,
  });
}

class FfiConverterNuts {
  static Nuts lift(RustBuffer buf) {
    return FfiConverterNuts.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Nuts> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final nut04_lifted = FfiConverterNut04Settings.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nut04 = nut04_lifted.value;
    new_offset += nut04_lifted.bytesRead;
    final nut05_lifted = FfiConverterNut05Settings.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nut05 = nut05_lifted.value;
    new_offset += nut05_lifted.bytesRead;
    final nut07Supported_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nut07Supported = nut07Supported_lifted.value;
    new_offset += nut07Supported_lifted.bytesRead;
    final nut08Supported_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nut08Supported = nut08Supported_lifted.value;
    new_offset += nut08Supported_lifted.bytesRead;
    final nut09Supported_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nut09Supported = nut09Supported_lifted.value;
    new_offset += nut09Supported_lifted.bytesRead;
    final nut10Supported_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nut10Supported = nut10Supported_lifted.value;
    new_offset += nut10Supported_lifted.bytesRead;
    final nut11Supported_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nut11Supported = nut11Supported_lifted.value;
    new_offset += nut11Supported_lifted.bytesRead;
    final nut12Supported_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nut12Supported = nut12Supported_lifted.value;
    new_offset += nut12Supported_lifted.bytesRead;
    final nut14Supported_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nut14Supported = nut14Supported_lifted.value;
    new_offset += nut14Supported_lifted.bytesRead;
    final nut20Supported_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nut20Supported = nut20Supported_lifted.value;
    new_offset += nut20Supported_lifted.bytesRead;
    final nut21_lifted = FfiConverterOptionalClearAuthSettings.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nut21 = nut21_lifted.value;
    new_offset += nut21_lifted.bytesRead;
    final nut22_lifted = FfiConverterOptionalBlindAuthSettings.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final nut22 = nut22_lifted.value;
    new_offset += nut22_lifted.bytesRead;
    final mintUnits_lifted = FfiConverterSequenceCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final mintUnits = mintUnits_lifted.value;
    new_offset += mintUnits_lifted.bytesRead;
    final meltUnits_lifted = FfiConverterSequenceCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final meltUnits = meltUnits_lifted.value;
    new_offset += meltUnits_lifted.bytesRead;
    return LiftRetVal(
      Nuts(
        nut04: nut04,
        nut05: nut05,
        nut07Supported: nut07Supported,
        nut08Supported: nut08Supported,
        nut09Supported: nut09Supported,
        nut10Supported: nut10Supported,
        nut11Supported: nut11Supported,
        nut12Supported: nut12Supported,
        nut14Supported: nut14Supported,
        nut20Supported: nut20Supported,
        nut21: nut21,
        nut22: nut22,
        mintUnits: mintUnits,
        meltUnits: meltUnits,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(Nuts value) {
    final total_length =
        FfiConverterNut04Settings.allocationSize(value.nut04) +
        FfiConverterNut05Settings.allocationSize(value.nut05) +
        FfiConverterBool.allocationSize(value.nut07Supported) +
        FfiConverterBool.allocationSize(value.nut08Supported) +
        FfiConverterBool.allocationSize(value.nut09Supported) +
        FfiConverterBool.allocationSize(value.nut10Supported) +
        FfiConverterBool.allocationSize(value.nut11Supported) +
        FfiConverterBool.allocationSize(value.nut12Supported) +
        FfiConverterBool.allocationSize(value.nut14Supported) +
        FfiConverterBool.allocationSize(value.nut20Supported) +
        FfiConverterOptionalClearAuthSettings.allocationSize(value.nut21) +
        FfiConverterOptionalBlindAuthSettings.allocationSize(value.nut22) +
        FfiConverterSequenceCurrencyUnit.allocationSize(value.mintUnits) +
        FfiConverterSequenceCurrencyUnit.allocationSize(value.meltUnits) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(Nuts value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterNut04Settings.write(
      value.nut04,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterNut05Settings.write(
      value.nut05,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.nut07Supported,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.nut08Supported,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.nut09Supported,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.nut10Supported,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.nut11Supported,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.nut12Supported,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.nut14Supported,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.nut20Supported,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalClearAuthSettings.write(
      value.nut21,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalBlindAuthSettings.write(
      value.nut22,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSequenceCurrencyUnit.write(
      value.mintUnits,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSequenceCurrencyUnit.write(
      value.meltUnits,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(Nuts value) {
    return FfiConverterNut04Settings.allocationSize(value.nut04) +
        FfiConverterNut05Settings.allocationSize(value.nut05) +
        FfiConverterBool.allocationSize(value.nut07Supported) +
        FfiConverterBool.allocationSize(value.nut08Supported) +
        FfiConverterBool.allocationSize(value.nut09Supported) +
        FfiConverterBool.allocationSize(value.nut10Supported) +
        FfiConverterBool.allocationSize(value.nut11Supported) +
        FfiConverterBool.allocationSize(value.nut12Supported) +
        FfiConverterBool.allocationSize(value.nut14Supported) +
        FfiConverterBool.allocationSize(value.nut20Supported) +
        FfiConverterOptionalClearAuthSettings.allocationSize(value.nut21) +
        FfiConverterOptionalBlindAuthSettings.allocationSize(value.nut22) +
        FfiConverterSequenceCurrencyUnit.allocationSize(value.mintUnits) +
        FfiConverterSequenceCurrencyUnit.allocationSize(value.meltUnits) +
        0;
  }
}

class Proof {
  final Amount amount;
  final String secret;
  final String c;
  final String keysetId;
  final Witness? witness;
  final ProofDleq? dleq;
  Proof({
    required this.amount,
    required this.secret,
    required this.c,
    required this.keysetId,
    required this.witness,
    required this.dleq,
  });
}

class FfiConverterProof {
  static Proof lift(RustBuffer buf) {
    return FfiConverterProof.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Proof> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final amount_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amount = amount_lifted.value;
    new_offset += amount_lifted.bytesRead;
    final secret_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final secret = secret_lifted.value;
    new_offset += secret_lifted.bytesRead;
    final c_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final c = c_lifted.value;
    new_offset += c_lifted.bytesRead;
    final keysetId_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final keysetId = keysetId_lifted.value;
    new_offset += keysetId_lifted.bytesRead;
    final witness_lifted = FfiConverterOptionalWitness.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final witness = witness_lifted.value;
    new_offset += witness_lifted.bytesRead;
    final dleq_lifted = FfiConverterOptionalProofDleq.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final dleq = dleq_lifted.value;
    new_offset += dleq_lifted.bytesRead;
    return LiftRetVal(
      Proof(
        amount: amount,
        secret: secret,
        c: c,
        keysetId: keysetId,
        witness: witness,
        dleq: dleq,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(Proof value) {
    final total_length =
        FfiConverterAmount.allocationSize(value.amount) +
        FfiConverterString.allocationSize(value.secret) +
        FfiConverterString.allocationSize(value.c) +
        FfiConverterString.allocationSize(value.keysetId) +
        FfiConverterOptionalWitness.allocationSize(value.witness) +
        FfiConverterOptionalProofDleq.allocationSize(value.dleq) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(Proof value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterAmount.write(
      value.amount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.secret,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.c,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.keysetId,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalWitness.write(
      value.witness,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalProofDleq.write(
      value.dleq,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(Proof value) {
    return FfiConverterAmount.allocationSize(value.amount) +
        FfiConverterString.allocationSize(value.secret) +
        FfiConverterString.allocationSize(value.c) +
        FfiConverterString.allocationSize(value.keysetId) +
        FfiConverterOptionalWitness.allocationSize(value.witness) +
        FfiConverterOptionalProofDleq.allocationSize(value.dleq) +
        0;
  }
}

class ProofDleq {
  final String e;
  final String s;
  final String r;
  ProofDleq({required this.e, required this.s, required this.r});
}

class FfiConverterProofDleq {
  static ProofDleq lift(RustBuffer buf) {
    return FfiConverterProofDleq.read(buf.asUint8List()).value;
  }

  static LiftRetVal<ProofDleq> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final e_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final e = e_lifted.value;
    new_offset += e_lifted.bytesRead;
    final s_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final s = s_lifted.value;
    new_offset += s_lifted.bytesRead;
    final r_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final r = r_lifted.value;
    new_offset += r_lifted.bytesRead;
    return LiftRetVal(
      ProofDleq(e: e, s: s, r: r),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(ProofDleq value) {
    final total_length =
        FfiConverterString.allocationSize(value.e) +
        FfiConverterString.allocationSize(value.s) +
        FfiConverterString.allocationSize(value.r) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(ProofDleq value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.e,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.s,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.r,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(ProofDleq value) {
    return FfiConverterString.allocationSize(value.e) +
        FfiConverterString.allocationSize(value.s) +
        FfiConverterString.allocationSize(value.r) +
        0;
  }
}

class ProofInfo {
  final Proof proof;
  final PublicKey y;
  final MintUrl mintUrl;
  final ProofState state;
  final SpendingConditions? spendingCondition;
  final CurrencyUnit unit;
  final String? usedByOperation;
  final String? createdByOperation;
  ProofInfo({
    required this.proof,
    required this.y,
    required this.mintUrl,
    required this.state,
    required this.spendingCondition,
    required this.unit,
    required this.usedByOperation,
    required this.createdByOperation,
  });
}

class FfiConverterProofInfo {
  static ProofInfo lift(RustBuffer buf) {
    return FfiConverterProofInfo.read(buf.asUint8List()).value;
  }

  static LiftRetVal<ProofInfo> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final proof_lifted = FfiConverterProof.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final proof = proof_lifted.value;
    new_offset += proof_lifted.bytesRead;
    final y_lifted = FfiConverterPublicKey.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final y = y_lifted.value;
    new_offset += y_lifted.bytesRead;
    final mintUrl_lifted = FfiConverterMintUrl.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final mintUrl = mintUrl_lifted.value;
    new_offset += mintUrl_lifted.bytesRead;
    final state_lifted = FfiConverterProofState.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final state = state_lifted.value;
    new_offset += state_lifted.bytesRead;
    final spendingCondition_lifted =
        FfiConverterOptionalSpendingConditions.read(
          Uint8List.view(buf.buffer, new_offset),
        );
    final spendingCondition = spendingCondition_lifted.value;
    new_offset += spendingCondition_lifted.bytesRead;
    final unit_lifted = FfiConverterCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final usedByOperation_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final usedByOperation = usedByOperation_lifted.value;
    new_offset += usedByOperation_lifted.bytesRead;
    final createdByOperation_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final createdByOperation = createdByOperation_lifted.value;
    new_offset += createdByOperation_lifted.bytesRead;
    return LiftRetVal(
      ProofInfo(
        proof: proof,
        y: y,
        mintUrl: mintUrl,
        state: state,
        spendingCondition: spendingCondition,
        unit: unit,
        usedByOperation: usedByOperation,
        createdByOperation: createdByOperation,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(ProofInfo value) {
    final total_length =
        FfiConverterProof.allocationSize(value.proof) +
        FfiConverterPublicKey.allocationSize(value.y) +
        FfiConverterMintUrl.allocationSize(value.mintUrl) +
        FfiConverterProofState.allocationSize(value.state) +
        FfiConverterOptionalSpendingConditions.allocationSize(
          value.spendingCondition,
        ) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalString.allocationSize(value.usedByOperation) +
        FfiConverterOptionalString.allocationSize(value.createdByOperation) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(ProofInfo value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterProof.write(
      value.proof,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterPublicKey.write(
      value.y,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterMintUrl.write(
      value.mintUrl,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterProofState.write(
      value.state,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalSpendingConditions.write(
      value.spendingCondition,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.usedByOperation,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.createdByOperation,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(ProofInfo value) {
    return FfiConverterProof.allocationSize(value.proof) +
        FfiConverterPublicKey.allocationSize(value.y) +
        FfiConverterMintUrl.allocationSize(value.mintUrl) +
        FfiConverterProofState.allocationSize(value.state) +
        FfiConverterOptionalSpendingConditions.allocationSize(
          value.spendingCondition,
        ) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalString.allocationSize(value.usedByOperation) +
        FfiConverterOptionalString.allocationSize(value.createdByOperation) +
        0;
  }
}

class ProofStateUpdate {
  final String y;
  final ProofState state;
  final String? witness;
  ProofStateUpdate({
    required this.y,
    required this.state,
    required this.witness,
  });
}

class FfiConverterProofStateUpdate {
  static ProofStateUpdate lift(RustBuffer buf) {
    return FfiConverterProofStateUpdate.read(buf.asUint8List()).value;
  }

  static LiftRetVal<ProofStateUpdate> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final y_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final y = y_lifted.value;
    new_offset += y_lifted.bytesRead;
    final state_lifted = FfiConverterProofState.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final state = state_lifted.value;
    new_offset += state_lifted.bytesRead;
    final witness_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final witness = witness_lifted.value;
    new_offset += witness_lifted.bytesRead;
    return LiftRetVal(
      ProofStateUpdate(y: y, state: state, witness: witness),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(ProofStateUpdate value) {
    final total_length =
        FfiConverterString.allocationSize(value.y) +
        FfiConverterProofState.allocationSize(value.state) +
        FfiConverterOptionalString.allocationSize(value.witness) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(ProofStateUpdate value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.y,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterProofState.write(
      value.state,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.witness,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(ProofStateUpdate value) {
    return FfiConverterString.allocationSize(value.y) +
        FfiConverterProofState.allocationSize(value.state) +
        FfiConverterOptionalString.allocationSize(value.witness) +
        0;
  }
}

class ProtectedEndpoint {
  final String method;
  final String path;
  ProtectedEndpoint({required this.method, required this.path});
}

class FfiConverterProtectedEndpoint {
  static ProtectedEndpoint lift(RustBuffer buf) {
    return FfiConverterProtectedEndpoint.read(buf.asUint8List()).value;
  }

  static LiftRetVal<ProtectedEndpoint> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final method_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final method = method_lifted.value;
    new_offset += method_lifted.bytesRead;
    final path_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final path = path_lifted.value;
    new_offset += path_lifted.bytesRead;
    return LiftRetVal(
      ProtectedEndpoint(method: method, path: path),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(ProtectedEndpoint value) {
    final total_length =
        FfiConverterString.allocationSize(value.method) +
        FfiConverterString.allocationSize(value.path) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(ProtectedEndpoint value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.method,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.path,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(ProtectedEndpoint value) {
    return FfiConverterString.allocationSize(value.method) +
        FfiConverterString.allocationSize(value.path) +
        0;
  }
}

class PublicKey {
  final String hex;
  PublicKey({required this.hex});
}

class FfiConverterPublicKey {
  static PublicKey lift(RustBuffer buf) {
    return FfiConverterPublicKey.read(buf.asUint8List()).value;
  }

  static LiftRetVal<PublicKey> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final hex_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final hex = hex_lifted.value;
    new_offset += hex_lifted.bytesRead;
    return LiftRetVal(PublicKey(hex: hex), new_offset - buf.offsetInBytes);
  }

  static RustBuffer lower(PublicKey value) {
    final total_length = FfiConverterString.allocationSize(value.hex) + 0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(PublicKey value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.hex,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(PublicKey value) {
    return FfiConverterString.allocationSize(value.hex) + 0;
  }
}

class ReceiveOptions {
  final SplitTarget amountSplitTarget;
  final List<SecretKey> p2pkSigningKeys;
  final List<String> preimages;
  final Map<String, String> metadata;
  ReceiveOptions({
    required this.amountSplitTarget,
    required this.p2pkSigningKeys,
    required this.preimages,
    required this.metadata,
  });
}

class FfiConverterReceiveOptions {
  static ReceiveOptions lift(RustBuffer buf) {
    return FfiConverterReceiveOptions.read(buf.asUint8List()).value;
  }

  static LiftRetVal<ReceiveOptions> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final amountSplitTarget_lifted = FfiConverterSplitTarget.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amountSplitTarget = amountSplitTarget_lifted.value;
    new_offset += amountSplitTarget_lifted.bytesRead;
    final p2pkSigningKeys_lifted = FfiConverterSequenceSecretKey.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final p2pkSigningKeys = p2pkSigningKeys_lifted.value;
    new_offset += p2pkSigningKeys_lifted.bytesRead;
    final preimages_lifted = FfiConverterSequenceString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final preimages = preimages_lifted.value;
    new_offset += preimages_lifted.bytesRead;
    final metadata_lifted = FfiConverterMapStringToString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final metadata = metadata_lifted.value;
    new_offset += metadata_lifted.bytesRead;
    return LiftRetVal(
      ReceiveOptions(
        amountSplitTarget: amountSplitTarget,
        p2pkSigningKeys: p2pkSigningKeys,
        preimages: preimages,
        metadata: metadata,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(ReceiveOptions value) {
    final total_length =
        FfiConverterSplitTarget.allocationSize(value.amountSplitTarget) +
        FfiConverterSequenceSecretKey.allocationSize(value.p2pkSigningKeys) +
        FfiConverterSequenceString.allocationSize(value.preimages) +
        FfiConverterMapStringToString.allocationSize(value.metadata) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(ReceiveOptions value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterSplitTarget.write(
      value.amountSplitTarget,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSequenceSecretKey.write(
      value.p2pkSigningKeys,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSequenceString.write(
      value.preimages,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterMapStringToString.write(
      value.metadata,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(ReceiveOptions value) {
    return FfiConverterSplitTarget.allocationSize(value.amountSplitTarget) +
        FfiConverterSequenceSecretKey.allocationSize(value.p2pkSigningKeys) +
        FfiConverterSequenceString.allocationSize(value.preimages) +
        FfiConverterMapStringToString.allocationSize(value.metadata) +
        0;
  }
}

class RestoreOptions {
  final int timeoutSecs;
  RestoreOptions({required this.timeoutSecs});
}

class FfiConverterRestoreOptions {
  static RestoreOptions lift(RustBuffer buf) {
    return FfiConverterRestoreOptions.read(buf.asUint8List()).value;
  }

  static LiftRetVal<RestoreOptions> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final timeoutSecs_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final timeoutSecs = timeoutSecs_lifted.value;
    new_offset += timeoutSecs_lifted.bytesRead;
    return LiftRetVal(
      RestoreOptions(timeoutSecs: timeoutSecs),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(RestoreOptions value) {
    final total_length =
        FfiConverterUInt64.allocationSize(value.timeoutSecs) + 0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(RestoreOptions value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterUInt64.write(
      value.timeoutSecs,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(RestoreOptions value) {
    return FfiConverterUInt64.allocationSize(value.timeoutSecs) + 0;
  }
}

class RestoreResult {
  final MintBackup backup;
  final int mintCount;
  final int mintsAdded;
  RestoreResult({
    required this.backup,
    required this.mintCount,
    required this.mintsAdded,
  });
}

class FfiConverterRestoreResult {
  static RestoreResult lift(RustBuffer buf) {
    return FfiConverterRestoreResult.read(buf.asUint8List()).value;
  }

  static LiftRetVal<RestoreResult> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final backup_lifted = FfiConverterMintBackup.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final backup = backup_lifted.value;
    new_offset += backup_lifted.bytesRead;
    final mintCount_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final mintCount = mintCount_lifted.value;
    new_offset += mintCount_lifted.bytesRead;
    final mintsAdded_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final mintsAdded = mintsAdded_lifted.value;
    new_offset += mintsAdded_lifted.bytesRead;
    return LiftRetVal(
      RestoreResult(
        backup: backup,
        mintCount: mintCount,
        mintsAdded: mintsAdded,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(RestoreResult value) {
    final total_length =
        FfiConverterMintBackup.allocationSize(value.backup) +
        FfiConverterUInt64.allocationSize(value.mintCount) +
        FfiConverterUInt64.allocationSize(value.mintsAdded) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(RestoreResult value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterMintBackup.write(
      value.backup,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.mintCount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.mintsAdded,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(RestoreResult value) {
    return FfiConverterMintBackup.allocationSize(value.backup) +
        FfiConverterUInt64.allocationSize(value.mintCount) +
        FfiConverterUInt64.allocationSize(value.mintsAdded) +
        0;
  }
}

class Restored {
  final Amount spent;
  final Amount unspent;
  final Amount pending;
  Restored({required this.spent, required this.unspent, required this.pending});
}

class FfiConverterRestored {
  static Restored lift(RustBuffer buf) {
    return FfiConverterRestored.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Restored> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final spent_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final spent = spent_lifted.value;
    new_offset += spent_lifted.bytesRead;
    final unspent_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unspent = unspent_lifted.value;
    new_offset += unspent_lifted.bytesRead;
    final pending_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final pending = pending_lifted.value;
    new_offset += pending_lifted.bytesRead;
    return LiftRetVal(
      Restored(spent: spent, unspent: unspent, pending: pending),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(Restored value) {
    final total_length =
        FfiConverterAmount.allocationSize(value.spent) +
        FfiConverterAmount.allocationSize(value.unspent) +
        FfiConverterAmount.allocationSize(value.pending) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(Restored value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterAmount.write(
      value.spent,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.unspent,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.pending,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(Restored value) {
    return FfiConverterAmount.allocationSize(value.spent) +
        FfiConverterAmount.allocationSize(value.unspent) +
        FfiConverterAmount.allocationSize(value.pending) +
        0;
  }
}

class SecretKey {
  final String hex;
  SecretKey({required this.hex});
}

class FfiConverterSecretKey {
  static SecretKey lift(RustBuffer buf) {
    return FfiConverterSecretKey.read(buf.asUint8List()).value;
  }

  static LiftRetVal<SecretKey> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final hex_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final hex = hex_lifted.value;
    new_offset += hex_lifted.bytesRead;
    return LiftRetVal(SecretKey(hex: hex), new_offset - buf.offsetInBytes);
  }

  static RustBuffer lower(SecretKey value) {
    final total_length = FfiConverterString.allocationSize(value.hex) + 0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(SecretKey value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.hex,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(SecretKey value) {
    return FfiConverterString.allocationSize(value.hex) + 0;
  }
}

class SendMemo {
  final String memo;
  final bool includeMemo;
  SendMemo({required this.memo, required this.includeMemo});
}

class FfiConverterSendMemo {
  static SendMemo lift(RustBuffer buf) {
    return FfiConverterSendMemo.read(buf.asUint8List()).value;
  }

  static LiftRetVal<SendMemo> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final memo_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final memo = memo_lifted.value;
    new_offset += memo_lifted.bytesRead;
    final includeMemo_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final includeMemo = includeMemo_lifted.value;
    new_offset += includeMemo_lifted.bytesRead;
    return LiftRetVal(
      SendMemo(memo: memo, includeMemo: includeMemo),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(SendMemo value) {
    final total_length =
        FfiConverterString.allocationSize(value.memo) +
        FfiConverterBool.allocationSize(value.includeMemo) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(SendMemo value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.memo,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.includeMemo,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(SendMemo value) {
    return FfiConverterString.allocationSize(value.memo) +
        FfiConverterBool.allocationSize(value.includeMemo) +
        0;
  }
}

class SendOptions {
  final SendMemo? memo;
  final SpendingConditions? conditions;
  final SplitTarget amountSplitTarget;
  final SendKind sendKind;
  final bool includeFee;
  final int? maxProofs;
  final Map<String, String> metadata;
  SendOptions({
    required this.memo,
    required this.conditions,
    required this.amountSplitTarget,
    required this.sendKind,
    required this.includeFee,
    required this.maxProofs,
    required this.metadata,
  });
}

class FfiConverterSendOptions {
  static SendOptions lift(RustBuffer buf) {
    return FfiConverterSendOptions.read(buf.asUint8List()).value;
  }

  static LiftRetVal<SendOptions> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final memo_lifted = FfiConverterOptionalSendMemo.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final memo = memo_lifted.value;
    new_offset += memo_lifted.bytesRead;
    final conditions_lifted = FfiConverterOptionalSpendingConditions.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final conditions = conditions_lifted.value;
    new_offset += conditions_lifted.bytesRead;
    final amountSplitTarget_lifted = FfiConverterSplitTarget.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amountSplitTarget = amountSplitTarget_lifted.value;
    new_offset += amountSplitTarget_lifted.bytesRead;
    final sendKind_lifted = FfiConverterSendKind.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final sendKind = sendKind_lifted.value;
    new_offset += sendKind_lifted.bytesRead;
    final includeFee_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final includeFee = includeFee_lifted.value;
    new_offset += includeFee_lifted.bytesRead;
    final maxProofs_lifted = FfiConverterOptionalUInt32.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final maxProofs = maxProofs_lifted.value;
    new_offset += maxProofs_lifted.bytesRead;
    final metadata_lifted = FfiConverterMapStringToString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final metadata = metadata_lifted.value;
    new_offset += metadata_lifted.bytesRead;
    return LiftRetVal(
      SendOptions(
        memo: memo,
        conditions: conditions,
        amountSplitTarget: amountSplitTarget,
        sendKind: sendKind,
        includeFee: includeFee,
        maxProofs: maxProofs,
        metadata: metadata,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(SendOptions value) {
    final total_length =
        FfiConverterOptionalSendMemo.allocationSize(value.memo) +
        FfiConverterOptionalSpendingConditions.allocationSize(
          value.conditions,
        ) +
        FfiConverterSplitTarget.allocationSize(value.amountSplitTarget) +
        FfiConverterSendKind.allocationSize(value.sendKind) +
        FfiConverterBool.allocationSize(value.includeFee) +
        FfiConverterOptionalUInt32.allocationSize(value.maxProofs) +
        FfiConverterMapStringToString.allocationSize(value.metadata) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(SendOptions value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterOptionalSendMemo.write(
      value.memo,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalSpendingConditions.write(
      value.conditions,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSplitTarget.write(
      value.amountSplitTarget,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSendKind.write(
      value.sendKind,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterBool.write(
      value.includeFee,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalUInt32.write(
      value.maxProofs,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterMapStringToString.write(
      value.metadata,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(SendOptions value) {
    return FfiConverterOptionalSendMemo.allocationSize(value.memo) +
        FfiConverterOptionalSpendingConditions.allocationSize(
          value.conditions,
        ) +
        FfiConverterSplitTarget.allocationSize(value.amountSplitTarget) +
        FfiConverterSendKind.allocationSize(value.sendKind) +
        FfiConverterBool.allocationSize(value.includeFee) +
        FfiConverterOptionalUInt32.allocationSize(value.maxProofs) +
        FfiConverterMapStringToString.allocationSize(value.metadata) +
        0;
  }
}

class SubscribeParams {
  final SubscriptionKind kind;
  final List<String> filters;
  final String? id;
  SubscribeParams({
    required this.kind,
    required this.filters,
    required this.id,
  });
}

class FfiConverterSubscribeParams {
  static SubscribeParams lift(RustBuffer buf) {
    return FfiConverterSubscribeParams.read(buf.asUint8List()).value;
  }

  static LiftRetVal<SubscribeParams> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final kind_lifted = FfiConverterSubscriptionKind.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final kind = kind_lifted.value;
    new_offset += kind_lifted.bytesRead;
    final filters_lifted = FfiConverterSequenceString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final filters = filters_lifted.value;
    new_offset += filters_lifted.bytesRead;
    final id_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final id = id_lifted.value;
    new_offset += id_lifted.bytesRead;
    return LiftRetVal(
      SubscribeParams(kind: kind, filters: filters, id: id),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(SubscribeParams value) {
    final total_length =
        FfiConverterSubscriptionKind.allocationSize(value.kind) +
        FfiConverterSequenceString.allocationSize(value.filters) +
        FfiConverterOptionalString.allocationSize(value.id) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(SubscribeParams value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterSubscriptionKind.write(
      value.kind,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSequenceString.write(
      value.filters,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.id,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(SubscribeParams value) {
    return FfiConverterSubscriptionKind.allocationSize(value.kind) +
        FfiConverterSequenceString.allocationSize(value.filters) +
        FfiConverterOptionalString.allocationSize(value.id) +
        0;
  }
}

class SupportedSettings {
  final bool supported;
  SupportedSettings({required this.supported});
}

class FfiConverterSupportedSettings {
  static SupportedSettings lift(RustBuffer buf) {
    return FfiConverterSupportedSettings.read(buf.asUint8List()).value;
  }

  static LiftRetVal<SupportedSettings> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final supported_lifted = FfiConverterBool.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final supported = supported_lifted.value;
    new_offset += supported_lifted.bytesRead;
    return LiftRetVal(
      SupportedSettings(supported: supported),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(SupportedSettings value) {
    final total_length = FfiConverterBool.allocationSize(value.supported) + 0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(SupportedSettings value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterBool.write(
      value.supported,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(SupportedSettings value) {
    return FfiConverterBool.allocationSize(value.supported) + 0;
  }
}

class TokenData {
  final MintUrl mintUrl;
  final List<Proof> proofs;
  final String? memo;
  final Amount value;
  final CurrencyUnit unit;
  final Amount? redeemFee;
  TokenData({
    required this.mintUrl,
    required this.proofs,
    required this.memo,
    required this.value,
    required this.unit,
    required this.redeemFee,
  });
}

class FfiConverterTokenData {
  static TokenData lift(RustBuffer buf) {
    return FfiConverterTokenData.read(buf.asUint8List()).value;
  }

  static LiftRetVal<TokenData> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final mintUrl_lifted = FfiConverterMintUrl.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final mintUrl = mintUrl_lifted.value;
    new_offset += mintUrl_lifted.bytesRead;
    final proofs_lifted = FfiConverterSequenceProof.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final proofs = proofs_lifted.value;
    new_offset += proofs_lifted.bytesRead;
    final memo_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final memo = memo_lifted.value;
    new_offset += memo_lifted.bytesRead;
    final value_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final value = value_lifted.value;
    new_offset += value_lifted.bytesRead;
    final unit_lifted = FfiConverterCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final redeemFee_lifted = FfiConverterOptionalAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final redeemFee = redeemFee_lifted.value;
    new_offset += redeemFee_lifted.bytesRead;
    return LiftRetVal(
      TokenData(
        mintUrl: mintUrl,
        proofs: proofs,
        memo: memo,
        value: value,
        unit: unit,
        redeemFee: redeemFee,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(TokenData value) {
    final total_length =
        FfiConverterMintUrl.allocationSize(value.mintUrl) +
        FfiConverterSequenceProof.allocationSize(value.proofs) +
        FfiConverterOptionalString.allocationSize(value.memo) +
        FfiConverterAmount.allocationSize(value.value) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalAmount.allocationSize(value.redeemFee) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(TokenData value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterMintUrl.write(
      value.mintUrl,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSequenceProof.write(
      value.proofs,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.memo,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.value,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalAmount.write(
      value.redeemFee,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(TokenData value) {
    return FfiConverterMintUrl.allocationSize(value.mintUrl) +
        FfiConverterSequenceProof.allocationSize(value.proofs) +
        FfiConverterOptionalString.allocationSize(value.memo) +
        FfiConverterAmount.allocationSize(value.value) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterOptionalAmount.allocationSize(value.redeemFee) +
        0;
  }
}

class Transaction {
  final TransactionId id;
  final MintUrl mintUrl;
  final TransactionDirection direction;
  final Amount amount;
  final Amount fee;
  final CurrencyUnit unit;
  final List<PublicKey> ys;
  final int timestamp;
  final String? memo;
  final Map<String, String> metadata;
  final String? quoteId;
  final String? paymentRequest;
  final String? paymentProof;
  final PaymentMethod? paymentMethod;
  final String? sagaId;
  Transaction({
    required this.id,
    required this.mintUrl,
    required this.direction,
    required this.amount,
    required this.fee,
    required this.unit,
    required this.ys,
    required this.timestamp,
    required this.memo,
    required this.metadata,
    required this.quoteId,
    required this.paymentRequest,
    required this.paymentProof,
    required this.paymentMethod,
    required this.sagaId,
  });
}

class FfiConverterTransaction {
  static Transaction lift(RustBuffer buf) {
    return FfiConverterTransaction.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Transaction> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final id_lifted = FfiConverterTransactionId.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final id = id_lifted.value;
    new_offset += id_lifted.bytesRead;
    final mintUrl_lifted = FfiConverterMintUrl.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final mintUrl = mintUrl_lifted.value;
    new_offset += mintUrl_lifted.bytesRead;
    final direction_lifted = FfiConverterTransactionDirection.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final direction = direction_lifted.value;
    new_offset += direction_lifted.bytesRead;
    final amount_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amount = amount_lifted.value;
    new_offset += amount_lifted.bytesRead;
    final fee_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final fee = fee_lifted.value;
    new_offset += fee_lifted.bytesRead;
    final unit_lifted = FfiConverterCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    final ys_lifted = FfiConverterSequencePublicKey.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final ys = ys_lifted.value;
    new_offset += ys_lifted.bytesRead;
    final timestamp_lifted = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final timestamp = timestamp_lifted.value;
    new_offset += timestamp_lifted.bytesRead;
    final memo_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final memo = memo_lifted.value;
    new_offset += memo_lifted.bytesRead;
    final metadata_lifted = FfiConverterMapStringToString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final metadata = metadata_lifted.value;
    new_offset += metadata_lifted.bytesRead;
    final quoteId_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final quoteId = quoteId_lifted.value;
    new_offset += quoteId_lifted.bytesRead;
    final paymentRequest_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final paymentRequest = paymentRequest_lifted.value;
    new_offset += paymentRequest_lifted.bytesRead;
    final paymentProof_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final paymentProof = paymentProof_lifted.value;
    new_offset += paymentProof_lifted.bytesRead;
    final paymentMethod_lifted = FfiConverterOptionalPaymentMethod.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final paymentMethod = paymentMethod_lifted.value;
    new_offset += paymentMethod_lifted.bytesRead;
    final sagaId_lifted = FfiConverterOptionalString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final sagaId = sagaId_lifted.value;
    new_offset += sagaId_lifted.bytesRead;
    return LiftRetVal(
      Transaction(
        id: id,
        mintUrl: mintUrl,
        direction: direction,
        amount: amount,
        fee: fee,
        unit: unit,
        ys: ys,
        timestamp: timestamp,
        memo: memo,
        metadata: metadata,
        quoteId: quoteId,
        paymentRequest: paymentRequest,
        paymentProof: paymentProof,
        paymentMethod: paymentMethod,
        sagaId: sagaId,
      ),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(Transaction value) {
    final total_length =
        FfiConverterTransactionId.allocationSize(value.id) +
        FfiConverterMintUrl.allocationSize(value.mintUrl) +
        FfiConverterTransactionDirection.allocationSize(value.direction) +
        FfiConverterAmount.allocationSize(value.amount) +
        FfiConverterAmount.allocationSize(value.fee) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterSequencePublicKey.allocationSize(value.ys) +
        FfiConverterUInt64.allocationSize(value.timestamp) +
        FfiConverterOptionalString.allocationSize(value.memo) +
        FfiConverterMapStringToString.allocationSize(value.metadata) +
        FfiConverterOptionalString.allocationSize(value.quoteId) +
        FfiConverterOptionalString.allocationSize(value.paymentRequest) +
        FfiConverterOptionalString.allocationSize(value.paymentProof) +
        FfiConverterOptionalPaymentMethod.allocationSize(value.paymentMethod) +
        FfiConverterOptionalString.allocationSize(value.sagaId) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(Transaction value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterTransactionId.write(
      value.id,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterMintUrl.write(
      value.mintUrl,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterTransactionDirection.write(
      value.direction,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.amount,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterAmount.write(
      value.fee,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSequencePublicKey.write(
      value.ys,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterUInt64.write(
      value.timestamp,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.memo,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterMapStringToString.write(
      value.metadata,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.quoteId,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.paymentRequest,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.paymentProof,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalPaymentMethod.write(
      value.paymentMethod,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalString.write(
      value.sagaId,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(Transaction value) {
    return FfiConverterTransactionId.allocationSize(value.id) +
        FfiConverterMintUrl.allocationSize(value.mintUrl) +
        FfiConverterTransactionDirection.allocationSize(value.direction) +
        FfiConverterAmount.allocationSize(value.amount) +
        FfiConverterAmount.allocationSize(value.fee) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        FfiConverterSequencePublicKey.allocationSize(value.ys) +
        FfiConverterUInt64.allocationSize(value.timestamp) +
        FfiConverterOptionalString.allocationSize(value.memo) +
        FfiConverterMapStringToString.allocationSize(value.metadata) +
        FfiConverterOptionalString.allocationSize(value.quoteId) +
        FfiConverterOptionalString.allocationSize(value.paymentRequest) +
        FfiConverterOptionalString.allocationSize(value.paymentProof) +
        FfiConverterOptionalPaymentMethod.allocationSize(value.paymentMethod) +
        FfiConverterOptionalString.allocationSize(value.sagaId) +
        0;
  }
}

class TransactionId {
  final String hex;
  TransactionId({required this.hex});
}

class FfiConverterTransactionId {
  static TransactionId lift(RustBuffer buf) {
    return FfiConverterTransactionId.read(buf.asUint8List()).value;
  }

  static LiftRetVal<TransactionId> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final hex_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final hex = hex_lifted.value;
    new_offset += hex_lifted.bytesRead;
    return LiftRetVal(TransactionId(hex: hex), new_offset - buf.offsetInBytes);
  }

  static RustBuffer lower(TransactionId value) {
    final total_length = FfiConverterString.allocationSize(value.hex) + 0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(TransactionId value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterString.write(
      value.hex,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(TransactionId value) {
    return FfiConverterString.allocationSize(value.hex) + 0;
  }
}

class Transport {
  final TransportType transportType;
  final String target;
  final List<List<String>> tags;
  Transport({
    required this.transportType,
    required this.target,
    required this.tags,
  });
}

class FfiConverterTransport {
  static Transport lift(RustBuffer buf) {
    return FfiConverterTransport.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Transport> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final transportType_lifted = FfiConverterTransportType.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final transportType = transportType_lifted.value;
    new_offset += transportType_lifted.bytesRead;
    final target_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final target = target_lifted.value;
    new_offset += target_lifted.bytesRead;
    final tags_lifted = FfiConverterSequenceSequenceString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final tags = tags_lifted.value;
    new_offset += tags_lifted.bytesRead;
    return LiftRetVal(
      Transport(transportType: transportType, target: target, tags: tags),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(Transport value) {
    final total_length =
        FfiConverterTransportType.allocationSize(value.transportType) +
        FfiConverterString.allocationSize(value.target) +
        FfiConverterSequenceSequenceString.allocationSize(value.tags) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(Transport value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterTransportType.write(
      value.transportType,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      value.target,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterSequenceSequenceString.write(
      value.tags,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(Transport value) {
    return FfiConverterTransportType.allocationSize(value.transportType) +
        FfiConverterString.allocationSize(value.target) +
        FfiConverterSequenceSequenceString.allocationSize(value.tags) +
        0;
  }
}

class WalletConfig {
  final int? targetProofCount;
  WalletConfig({required this.targetProofCount});
}

class FfiConverterWalletConfig {
  static WalletConfig lift(RustBuffer buf) {
    return FfiConverterWalletConfig.read(buf.asUint8List()).value;
  }

  static LiftRetVal<WalletConfig> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final targetProofCount_lifted = FfiConverterOptionalUInt32.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final targetProofCount = targetProofCount_lifted.value;
    new_offset += targetProofCount_lifted.bytesRead;
    return LiftRetVal(
      WalletConfig(targetProofCount: targetProofCount),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(WalletConfig value) {
    final total_length =
        FfiConverterOptionalUInt32.allocationSize(value.targetProofCount) + 0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(WalletConfig value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterOptionalUInt32.write(
      value.targetProofCount,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(WalletConfig value) {
    return FfiConverterOptionalUInt32.allocationSize(value.targetProofCount) +
        0;
  }
}

class WalletKey {
  final MintUrl mintUrl;
  final CurrencyUnit unit;
  WalletKey({required this.mintUrl, required this.unit});
}

class FfiConverterWalletKey {
  static WalletKey lift(RustBuffer buf) {
    return FfiConverterWalletKey.read(buf.asUint8List()).value;
  }

  static LiftRetVal<WalletKey> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final mintUrl_lifted = FfiConverterMintUrl.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final mintUrl = mintUrl_lifted.value;
    new_offset += mintUrl_lifted.bytesRead;
    final unit_lifted = FfiConverterCurrencyUnit.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    return LiftRetVal(
      WalletKey(mintUrl: mintUrl, unit: unit),
      new_offset - buf.offsetInBytes,
    );
  }

  static RustBuffer lower(WalletKey value) {
    final total_length =
        FfiConverterMintUrl.allocationSize(value.mintUrl) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        0;
    final buf = Uint8List(total_length);
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int write(WalletKey value, Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    new_offset += FfiConverterMintUrl.write(
      value.mintUrl,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterCurrencyUnit.write(
      value.unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset - buf.offsetInBytes;
  }

  static int allocationSize(WalletKey value) {
    return FfiConverterMintUrl.allocationSize(value.mintUrl) +
        FfiConverterCurrencyUnit.allocationSize(value.unit) +
        0;
  }
}

abstract class CurrencyUnit {
  RustBuffer lower();
  int allocationSize();
  int write(Uint8List buf);
}

class FfiConverterCurrencyUnit {
  static CurrencyUnit lift(RustBuffer buffer) {
    return FfiConverterCurrencyUnit.read(buffer.asUint8List()).value;
  }

  static LiftRetVal<CurrencyUnit> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    final subview = Uint8List.view(buf.buffer, buf.offsetInBytes + 4);
    switch (index) {
      case 1:
        final lifted = SatCurrencyUnit.read(subview);
        return LiftRetVal<CurrencyUnit>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 2:
        final lifted = MsatCurrencyUnit.read(subview);
        return LiftRetVal<CurrencyUnit>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 3:
        final lifted = UsdCurrencyUnit.read(subview);
        return LiftRetVal<CurrencyUnit>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 4:
        final lifted = EurCurrencyUnit.read(subview);
        return LiftRetVal<CurrencyUnit>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 5:
        final lifted = AuthCurrencyUnit.read(subview);
        return LiftRetVal<CurrencyUnit>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 6:
        final lifted = CustomCurrencyUnit.read(subview);
        return LiftRetVal<CurrencyUnit>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static RustBuffer lower(CurrencyUnit value) {
    return value.lower();
  }

  static int allocationSize(CurrencyUnit value) {
    return value.allocationSize();
  }

  static int write(CurrencyUnit value, Uint8List buf) {
    return value.write(buf) - buf.offsetInBytes;
  }
}

class SatCurrencyUnit extends CurrencyUnit {
  SatCurrencyUnit();
  SatCurrencyUnit._();
  static LiftRetVal<SatCurrencyUnit> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    return LiftRetVal(SatCurrencyUnit._(), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 1);
    int new_offset = buf.offsetInBytes + 4;
    return new_offset;
  }
}

class MsatCurrencyUnit extends CurrencyUnit {
  MsatCurrencyUnit();
  MsatCurrencyUnit._();
  static LiftRetVal<MsatCurrencyUnit> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    return LiftRetVal(MsatCurrencyUnit._(), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 2);
    int new_offset = buf.offsetInBytes + 4;
    return new_offset;
  }
}

class UsdCurrencyUnit extends CurrencyUnit {
  UsdCurrencyUnit();
  UsdCurrencyUnit._();
  static LiftRetVal<UsdCurrencyUnit> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    return LiftRetVal(UsdCurrencyUnit._(), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 3);
    int new_offset = buf.offsetInBytes + 4;
    return new_offset;
  }
}

class EurCurrencyUnit extends CurrencyUnit {
  EurCurrencyUnit();
  EurCurrencyUnit._();
  static LiftRetVal<EurCurrencyUnit> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    return LiftRetVal(EurCurrencyUnit._(), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 4);
    int new_offset = buf.offsetInBytes + 4;
    return new_offset;
  }
}

class AuthCurrencyUnit extends CurrencyUnit {
  AuthCurrencyUnit();
  AuthCurrencyUnit._();
  static LiftRetVal<AuthCurrencyUnit> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    return LiftRetVal(AuthCurrencyUnit._(), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 5);
    int new_offset = buf.offsetInBytes + 4;
    return new_offset;
  }
}

class CustomCurrencyUnit extends CurrencyUnit {
  final String unit;
  CustomCurrencyUnit(String this.unit);
  CustomCurrencyUnit._(String this.unit);
  static LiftRetVal<CustomCurrencyUnit> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final unit_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final unit = unit_lifted.value;
    new_offset += unit_lifted.bytesRead;
    return LiftRetVal(CustomCurrencyUnit._(unit), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterString.allocationSize(unit) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 6);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterString.write(
      unit,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

abstract class FfiException implements Exception {
  RustBuffer lower();
  int allocationSize();
  int write(Uint8List buf);
}

class FfiConverterFfiException {
  static FfiException lift(RustBuffer buffer) {
    return FfiConverterFfiException.read(buffer.asUint8List()).value;
  }

  static LiftRetVal<FfiException> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    final subview = Uint8List.view(buf.buffer, buf.offsetInBytes + 4);
    switch (index) {
      case 1:
        final lifted = CdkFfiException.read(subview);
        return LiftRetVal<FfiException>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 2:
        final lifted = InternalFfiException.read(subview);
        return LiftRetVal<FfiException>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static RustBuffer lower(FfiException value) {
    return value.lower();
  }

  static int allocationSize(FfiException value) {
    return value.allocationSize();
  }

  static int write(FfiException value, Uint8List buf) {
    return value.write(buf) - buf.offsetInBytes;
  }
}

class CdkFfiException extends FfiException {
  final int code;
  final String errorMessage;
  CdkFfiException({required int this.code, required String this.errorMessage});
  CdkFfiException._(int this.code, String this.errorMessage);
  static LiftRetVal<CdkFfiException> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final code_lifted = FfiConverterUInt32.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final code = code_lifted.value;
    new_offset += code_lifted.bytesRead;
    final errorMessage_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final errorMessage = errorMessage_lifted.value;
    new_offset += errorMessage_lifted.bytesRead;
    return LiftRetVal(CdkFfiException._(code, errorMessage), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterUInt32.allocationSize(code) +
        FfiConverterString.allocationSize(errorMessage) +
        4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 1);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterUInt32.write(
      code,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterString.write(
      errorMessage,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }

  @override
  String toString() {
    return "CdkFfiException($code, $errorMessage)";
  }
}

class InternalFfiException extends FfiException {
  final String errorMessage;
  InternalFfiException(String this.errorMessage);
  InternalFfiException._(String this.errorMessage);
  static LiftRetVal<InternalFfiException> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final errorMessage_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final errorMessage = errorMessage_lifted.value;
    new_offset += errorMessage_lifted.bytesRead;
    return LiftRetVal(InternalFfiException._(errorMessage), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterString.allocationSize(errorMessage) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 2);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterString.write(
      errorMessage,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }

  @override
  String toString() {
    return "InternalFfiException($errorMessage)";
  }
}

class FfiExceptionErrorHandler extends UniffiRustCallStatusErrorHandler {
  @override
  Exception lift(RustBuffer errorBuf) {
    return FfiConverterFfiException.lift(errorBuf);
  }
}

final FfiExceptionErrorHandler ffiExceptionErrorHandler =
    FfiExceptionErrorHandler();

abstract class MeltOptions {
  RustBuffer lower();
  int allocationSize();
  int write(Uint8List buf);
}

class FfiConverterMeltOptions {
  static MeltOptions lift(RustBuffer buffer) {
    return FfiConverterMeltOptions.read(buffer.asUint8List()).value;
  }

  static LiftRetVal<MeltOptions> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    final subview = Uint8List.view(buf.buffer, buf.offsetInBytes + 4);
    switch (index) {
      case 1:
        final lifted = MppMeltOptions.read(subview);
        return LiftRetVal<MeltOptions>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 2:
        final lifted = AmountlessMeltOptions.read(subview);
        return LiftRetVal<MeltOptions>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static RustBuffer lower(MeltOptions value) {
    return value.lower();
  }

  static int allocationSize(MeltOptions value) {
    return value.allocationSize();
  }

  static int write(MeltOptions value, Uint8List buf) {
    return value.write(buf) - buf.offsetInBytes;
  }
}

class MppMeltOptions extends MeltOptions {
  final Amount amount;
  MppMeltOptions(Amount this.amount);
  MppMeltOptions._(Amount this.amount);
  static LiftRetVal<MppMeltOptions> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final amount_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amount = amount_lifted.value;
    new_offset += amount_lifted.bytesRead;
    return LiftRetVal(MppMeltOptions._(amount), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterAmount.allocationSize(amount) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 1);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterAmount.write(
      amount,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

class AmountlessMeltOptions extends MeltOptions {
  final Amount amountMsat;
  AmountlessMeltOptions(Amount this.amountMsat);
  AmountlessMeltOptions._(Amount this.amountMsat);
  static LiftRetVal<AmountlessMeltOptions> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final amountMsat_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amountMsat = amountMsat_lifted.value;
    new_offset += amountMsat_lifted.bytesRead;
    return LiftRetVal(AmountlessMeltOptions._(amountMsat), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterAmount.allocationSize(amountMsat) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 2);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterAmount.write(
      amountMsat,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

abstract class NotificationPayload {
  RustBuffer lower();
  int allocationSize();
  int write(Uint8List buf);
}

class FfiConverterNotificationPayload {
  static NotificationPayload lift(RustBuffer buffer) {
    return FfiConverterNotificationPayload.read(buffer.asUint8List()).value;
  }

  static LiftRetVal<NotificationPayload> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    final subview = Uint8List.view(buf.buffer, buf.offsetInBytes + 4);
    switch (index) {
      case 1:
        final lifted = ProofStateNotificationPayload.read(subview);
        return LiftRetVal<NotificationPayload>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 2:
        final lifted = MintQuoteUpdateNotificationPayload.read(subview);
        return LiftRetVal<NotificationPayload>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 3:
        final lifted = MeltQuoteUpdateNotificationPayload.read(subview);
        return LiftRetVal<NotificationPayload>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static RustBuffer lower(NotificationPayload value) {
    return value.lower();
  }

  static int allocationSize(NotificationPayload value) {
    return value.allocationSize();
  }

  static int write(NotificationPayload value, Uint8List buf) {
    return value.write(buf) - buf.offsetInBytes;
  }
}

class ProofStateNotificationPayload extends NotificationPayload {
  final List<ProofStateUpdate> proofStates;
  ProofStateNotificationPayload(List<ProofStateUpdate> this.proofStates);
  ProofStateNotificationPayload._(List<ProofStateUpdate> this.proofStates);
  static LiftRetVal<ProofStateNotificationPayload> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final proofStates_lifted = FfiConverterSequenceProofStateUpdate.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final proofStates = proofStates_lifted.value;
    new_offset += proofStates_lifted.bytesRead;
    return LiftRetVal(ProofStateNotificationPayload._(proofStates), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterSequenceProofStateUpdate.allocationSize(proofStates) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 1);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterSequenceProofStateUpdate.write(
      proofStates,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

class MintQuoteUpdateNotificationPayload extends NotificationPayload {
  final MintQuoteBolt11Response quote;
  MintQuoteUpdateNotificationPayload(MintQuoteBolt11Response this.quote);
  MintQuoteUpdateNotificationPayload._(MintQuoteBolt11Response this.quote);
  static LiftRetVal<MintQuoteUpdateNotificationPayload> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final quote_lifted = FfiConverterMintQuoteBolt11Response.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final quote = quote_lifted.value;
    new_offset += quote_lifted.bytesRead;
    return LiftRetVal(MintQuoteUpdateNotificationPayload._(quote), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterMintQuoteBolt11Response.allocationSize(quote) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 2);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterMintQuoteBolt11Response.write(
      quote,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

class MeltQuoteUpdateNotificationPayload extends NotificationPayload {
  final MeltQuoteBolt11Response quote;
  MeltQuoteUpdateNotificationPayload(MeltQuoteBolt11Response this.quote);
  MeltQuoteUpdateNotificationPayload._(MeltQuoteBolt11Response this.quote);
  static LiftRetVal<MeltQuoteUpdateNotificationPayload> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final quote_lifted = FfiConverterMeltQuoteBolt11Response.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final quote = quote_lifted.value;
    new_offset += quote_lifted.bytesRead;
    return LiftRetVal(MeltQuoteUpdateNotificationPayload._(quote), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterMeltQuoteBolt11Response.allocationSize(quote) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 3);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterMeltQuoteBolt11Response.write(
      quote,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

abstract class PaymentMethod {
  RustBuffer lower();
  int allocationSize();
  int write(Uint8List buf);
}

class FfiConverterPaymentMethod {
  static PaymentMethod lift(RustBuffer buffer) {
    return FfiConverterPaymentMethod.read(buffer.asUint8List()).value;
  }

  static LiftRetVal<PaymentMethod> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    final subview = Uint8List.view(buf.buffer, buf.offsetInBytes + 4);
    switch (index) {
      case 1:
        final lifted = Bolt11PaymentMethod.read(subview);
        return LiftRetVal<PaymentMethod>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 2:
        final lifted = Bolt12PaymentMethod.read(subview);
        return LiftRetVal<PaymentMethod>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 3:
        final lifted = CustomPaymentMethod.read(subview);
        return LiftRetVal<PaymentMethod>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static RustBuffer lower(PaymentMethod value) {
    return value.lower();
  }

  static int allocationSize(PaymentMethod value) {
    return value.allocationSize();
  }

  static int write(PaymentMethod value, Uint8List buf) {
    return value.write(buf) - buf.offsetInBytes;
  }
}

class Bolt11PaymentMethod extends PaymentMethod {
  Bolt11PaymentMethod();
  Bolt11PaymentMethod._();
  static LiftRetVal<Bolt11PaymentMethod> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    return LiftRetVal(Bolt11PaymentMethod._(), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 1);
    int new_offset = buf.offsetInBytes + 4;
    return new_offset;
  }
}

class Bolt12PaymentMethod extends PaymentMethod {
  Bolt12PaymentMethod();
  Bolt12PaymentMethod._();
  static LiftRetVal<Bolt12PaymentMethod> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    return LiftRetVal(Bolt12PaymentMethod._(), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 2);
    int new_offset = buf.offsetInBytes + 4;
    return new_offset;
  }
}

class CustomPaymentMethod extends PaymentMethod {
  final String method;
  CustomPaymentMethod(String this.method);
  CustomPaymentMethod._(String this.method);
  static LiftRetVal<CustomPaymentMethod> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final method_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final method = method_lifted.value;
    new_offset += method_lifted.bytesRead;
    return LiftRetVal(CustomPaymentMethod._(method), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterString.allocationSize(method) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 3);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterString.write(
      method,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

enum PaymentType { bolt11, bolt12 }

class FfiConverterPaymentType {
  static LiftRetVal<PaymentType> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    switch (index) {
      case 1:
        return LiftRetVal(PaymentType.bolt11, 4);
      case 2:
        return LiftRetVal(PaymentType.bolt12, 4);
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static PaymentType lift(RustBuffer buffer) {
    return FfiConverterPaymentType.read(buffer.asUint8List()).value;
  }

  static RustBuffer lower(PaymentType input) {
    return toRustBuffer(createUint8ListFromInt(input.index + 1));
  }

  static int allocationSize(PaymentType _value) {
    return 4;
  }

  static int write(PaymentType value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.index + 1);
    return 4;
  }
}

enum ProofState { unspent, pending, spent, reserved, pendingSpent }

class FfiConverterProofState {
  static LiftRetVal<ProofState> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    switch (index) {
      case 1:
        return LiftRetVal(ProofState.unspent, 4);
      case 2:
        return LiftRetVal(ProofState.pending, 4);
      case 3:
        return LiftRetVal(ProofState.spent, 4);
      case 4:
        return LiftRetVal(ProofState.reserved, 4);
      case 5:
        return LiftRetVal(ProofState.pendingSpent, 4);
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static ProofState lift(RustBuffer buffer) {
    return FfiConverterProofState.read(buffer.asUint8List()).value;
  }

  static RustBuffer lower(ProofState input) {
    return toRustBuffer(createUint8ListFromInt(input.index + 1));
  }

  static int allocationSize(ProofState _value) {
    return 4;
  }

  static int write(ProofState value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.index + 1);
    return 4;
  }
}

enum QuoteState { unpaid, paid, pending, issued }

class FfiConverterQuoteState {
  static LiftRetVal<QuoteState> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    switch (index) {
      case 1:
        return LiftRetVal(QuoteState.unpaid, 4);
      case 2:
        return LiftRetVal(QuoteState.paid, 4);
      case 3:
        return LiftRetVal(QuoteState.pending, 4);
      case 4:
        return LiftRetVal(QuoteState.issued, 4);
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static QuoteState lift(RustBuffer buffer) {
    return FfiConverterQuoteState.read(buffer.asUint8List()).value;
  }

  static RustBuffer lower(QuoteState input) {
    return toRustBuffer(createUint8ListFromInt(input.index + 1));
  }

  static int allocationSize(QuoteState _value) {
    return 4;
  }

  static int write(QuoteState value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.index + 1);
    return 4;
  }
}

abstract class SendKind {
  RustBuffer lower();
  int allocationSize();
  int write(Uint8List buf);
}

class FfiConverterSendKind {
  static SendKind lift(RustBuffer buffer) {
    return FfiConverterSendKind.read(buffer.asUint8List()).value;
  }

  static LiftRetVal<SendKind> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    final subview = Uint8List.view(buf.buffer, buf.offsetInBytes + 4);
    switch (index) {
      case 1:
        final lifted = OnlineExactSendKind.read(subview);
        return LiftRetVal<SendKind>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 2:
        final lifted = OnlineToleranceSendKind.read(subview);
        return LiftRetVal<SendKind>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 3:
        final lifted = OfflineExactSendKind.read(subview);
        return LiftRetVal<SendKind>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 4:
        final lifted = OfflineToleranceSendKind.read(subview);
        return LiftRetVal<SendKind>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static RustBuffer lower(SendKind value) {
    return value.lower();
  }

  static int allocationSize(SendKind value) {
    return value.allocationSize();
  }

  static int write(SendKind value, Uint8List buf) {
    return value.write(buf) - buf.offsetInBytes;
  }
}

class OnlineExactSendKind extends SendKind {
  OnlineExactSendKind();
  OnlineExactSendKind._();
  static LiftRetVal<OnlineExactSendKind> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    return LiftRetVal(OnlineExactSendKind._(), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 1);
    int new_offset = buf.offsetInBytes + 4;
    return new_offset;
  }
}

class OnlineToleranceSendKind extends SendKind {
  final Amount tolerance;
  OnlineToleranceSendKind(Amount this.tolerance);
  OnlineToleranceSendKind._(Amount this.tolerance);
  static LiftRetVal<OnlineToleranceSendKind> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final tolerance_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final tolerance = tolerance_lifted.value;
    new_offset += tolerance_lifted.bytesRead;
    return LiftRetVal(OnlineToleranceSendKind._(tolerance), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterAmount.allocationSize(tolerance) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 2);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterAmount.write(
      tolerance,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

class OfflineExactSendKind extends SendKind {
  OfflineExactSendKind();
  OfflineExactSendKind._();
  static LiftRetVal<OfflineExactSendKind> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    return LiftRetVal(OfflineExactSendKind._(), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 3);
    int new_offset = buf.offsetInBytes + 4;
    return new_offset;
  }
}

class OfflineToleranceSendKind extends SendKind {
  final Amount tolerance;
  OfflineToleranceSendKind(Amount this.tolerance);
  OfflineToleranceSendKind._(Amount this.tolerance);
  static LiftRetVal<OfflineToleranceSendKind> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final tolerance_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final tolerance = tolerance_lifted.value;
    new_offset += tolerance_lifted.bytesRead;
    return LiftRetVal(OfflineToleranceSendKind._(tolerance), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterAmount.allocationSize(tolerance) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 4);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterAmount.write(
      tolerance,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

abstract class SpendingConditions {
  RustBuffer lower();
  int allocationSize();
  int write(Uint8List buf);
}

class FfiConverterSpendingConditions {
  static SpendingConditions lift(RustBuffer buffer) {
    return FfiConverterSpendingConditions.read(buffer.asUint8List()).value;
  }

  static LiftRetVal<SpendingConditions> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    final subview = Uint8List.view(buf.buffer, buf.offsetInBytes + 4);
    switch (index) {
      case 1:
        final lifted = P2pkSpendingConditions.read(subview);
        return LiftRetVal<SpendingConditions>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 2:
        final lifted = HtlcSpendingConditions.read(subview);
        return LiftRetVal<SpendingConditions>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static RustBuffer lower(SpendingConditions value) {
    return value.lower();
  }

  static int allocationSize(SpendingConditions value) {
    return value.allocationSize();
  }

  static int write(SpendingConditions value, Uint8List buf) {
    return value.write(buf) - buf.offsetInBytes;
  }
}

class P2pkSpendingConditions extends SpendingConditions {
  final String pubkey;
  final Conditions? conditions;
  P2pkSpendingConditions({
    required String this.pubkey,
    required Conditions? this.conditions,
  });
  P2pkSpendingConditions._(String this.pubkey, Conditions? this.conditions);
  static LiftRetVal<P2pkSpendingConditions> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final pubkey_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final pubkey = pubkey_lifted.value;
    new_offset += pubkey_lifted.bytesRead;
    final conditions_lifted = FfiConverterOptionalConditions.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final conditions = conditions_lifted.value;
    new_offset += conditions_lifted.bytesRead;
    return LiftRetVal(P2pkSpendingConditions._(pubkey, conditions), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterString.allocationSize(pubkey) +
        FfiConverterOptionalConditions.allocationSize(conditions) +
        4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 1);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterString.write(
      pubkey,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalConditions.write(
      conditions,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

class HtlcSpendingConditions extends SpendingConditions {
  final String hash;
  final Conditions? conditions;
  HtlcSpendingConditions({
    required String this.hash,
    required Conditions? this.conditions,
  });
  HtlcSpendingConditions._(String this.hash, Conditions? this.conditions);
  static LiftRetVal<HtlcSpendingConditions> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final hash_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final hash = hash_lifted.value;
    new_offset += hash_lifted.bytesRead;
    final conditions_lifted = FfiConverterOptionalConditions.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final conditions = conditions_lifted.value;
    new_offset += conditions_lifted.bytesRead;
    return LiftRetVal(HtlcSpendingConditions._(hash, conditions), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterString.allocationSize(hash) +
        FfiConverterOptionalConditions.allocationSize(conditions) +
        4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 2);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterString.write(
      hash,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalConditions.write(
      conditions,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

abstract class SplitTarget {
  RustBuffer lower();
  int allocationSize();
  int write(Uint8List buf);
}

class FfiConverterSplitTarget {
  static SplitTarget lift(RustBuffer buffer) {
    return FfiConverterSplitTarget.read(buffer.asUint8List()).value;
  }

  static LiftRetVal<SplitTarget> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    final subview = Uint8List.view(buf.buffer, buf.offsetInBytes + 4);
    switch (index) {
      case 1:
        final lifted = NoneSplitTarget.read(subview);
        return LiftRetVal<SplitTarget>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 2:
        final lifted = ValueSplitTarget.read(subview);
        return LiftRetVal<SplitTarget>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 3:
        final lifted = ValuesSplitTarget.read(subview);
        return LiftRetVal<SplitTarget>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static RustBuffer lower(SplitTarget value) {
    return value.lower();
  }

  static int allocationSize(SplitTarget value) {
    return value.allocationSize();
  }

  static int write(SplitTarget value, Uint8List buf) {
    return value.write(buf) - buf.offsetInBytes;
  }
}

class NoneSplitTarget extends SplitTarget {
  NoneSplitTarget();
  NoneSplitTarget._();
  static LiftRetVal<NoneSplitTarget> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    return LiftRetVal(NoneSplitTarget._(), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 1);
    int new_offset = buf.offsetInBytes + 4;
    return new_offset;
  }
}

class ValueSplitTarget extends SplitTarget {
  final Amount amount;
  ValueSplitTarget(Amount this.amount);
  ValueSplitTarget._(Amount this.amount);
  static LiftRetVal<ValueSplitTarget> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final amount_lifted = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amount = amount_lifted.value;
    new_offset += amount_lifted.bytesRead;
    return LiftRetVal(ValueSplitTarget._(amount), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterAmount.allocationSize(amount) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 2);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterAmount.write(
      amount,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

class ValuesSplitTarget extends SplitTarget {
  final List<Amount> amounts;
  ValuesSplitTarget(List<Amount> this.amounts);
  ValuesSplitTarget._(List<Amount> this.amounts);
  static LiftRetVal<ValuesSplitTarget> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final amounts_lifted = FfiConverterSequenceAmount.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final amounts = amounts_lifted.value;
    new_offset += amounts_lifted.bytesRead;
    return LiftRetVal(ValuesSplitTarget._(amounts), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterSequenceAmount.allocationSize(amounts) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 3);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterSequenceAmount.write(
      amounts,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

enum SubscriptionKind {
  bolt11MeltQuote,
  bolt11MintQuote,
  bolt12MintQuote,
  bolt12MeltQuote,
  proofState,
}

class FfiConverterSubscriptionKind {
  static LiftRetVal<SubscriptionKind> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    switch (index) {
      case 1:
        return LiftRetVal(SubscriptionKind.bolt11MeltQuote, 4);
      case 2:
        return LiftRetVal(SubscriptionKind.bolt11MintQuote, 4);
      case 3:
        return LiftRetVal(SubscriptionKind.bolt12MintQuote, 4);
      case 4:
        return LiftRetVal(SubscriptionKind.bolt12MeltQuote, 4);
      case 5:
        return LiftRetVal(SubscriptionKind.proofState, 4);
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static SubscriptionKind lift(RustBuffer buffer) {
    return FfiConverterSubscriptionKind.read(buffer.asUint8List()).value;
  }

  static RustBuffer lower(SubscriptionKind input) {
    return toRustBuffer(createUint8ListFromInt(input.index + 1));
  }

  static int allocationSize(SubscriptionKind _value) {
    return 4;
  }

  static int write(SubscriptionKind value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.index + 1);
    return 4;
  }
}

enum TransactionDirection { incoming, outgoing }

class FfiConverterTransactionDirection {
  static LiftRetVal<TransactionDirection> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    switch (index) {
      case 1:
        return LiftRetVal(TransactionDirection.incoming, 4);
      case 2:
        return LiftRetVal(TransactionDirection.outgoing, 4);
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static TransactionDirection lift(RustBuffer buffer) {
    return FfiConverterTransactionDirection.read(buffer.asUint8List()).value;
  }

  static RustBuffer lower(TransactionDirection input) {
    return toRustBuffer(createUint8ListFromInt(input.index + 1));
  }

  static int allocationSize(TransactionDirection _value) {
    return 4;
  }

  static int write(TransactionDirection value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.index + 1);
    return 4;
  }
}

enum TransportType { nostr, httpPost }

class FfiConverterTransportType {
  static LiftRetVal<TransportType> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    switch (index) {
      case 1:
        return LiftRetVal(TransportType.nostr, 4);
      case 2:
        return LiftRetVal(TransportType.httpPost, 4);
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static TransportType lift(RustBuffer buffer) {
    return FfiConverterTransportType.read(buffer.asUint8List()).value;
  }

  static RustBuffer lower(TransportType input) {
    return toRustBuffer(createUint8ListFromInt(input.index + 1));
  }

  static int allocationSize(TransportType _value) {
    return 4;
  }

  static int write(TransportType value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.index + 1);
    return 4;
  }
}

abstract class WalletDbBackend {
  RustBuffer lower();
  int allocationSize();
  int write(Uint8List buf);
}

class FfiConverterWalletDbBackend {
  static WalletDbBackend lift(RustBuffer buffer) {
    return FfiConverterWalletDbBackend.read(buffer.asUint8List()).value;
  }

  static LiftRetVal<WalletDbBackend> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    final subview = Uint8List.view(buf.buffer, buf.offsetInBytes + 4);
    switch (index) {
      case 1:
        final lifted = SqliteWalletDbBackend.read(subview);
        return LiftRetVal<WalletDbBackend>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 2:
        final lifted = PostgresWalletDbBackend.read(subview);
        return LiftRetVal<WalletDbBackend>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static RustBuffer lower(WalletDbBackend value) {
    return value.lower();
  }

  static int allocationSize(WalletDbBackend value) {
    return value.allocationSize();
  }

  static int write(WalletDbBackend value, Uint8List buf) {
    return value.write(buf) - buf.offsetInBytes;
  }
}

class SqliteWalletDbBackend extends WalletDbBackend {
  final String path;
  SqliteWalletDbBackend(String this.path);
  SqliteWalletDbBackend._(String this.path);
  static LiftRetVal<SqliteWalletDbBackend> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final path_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final path = path_lifted.value;
    new_offset += path_lifted.bytesRead;
    return LiftRetVal(SqliteWalletDbBackend._(path), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterString.allocationSize(path) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 1);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterString.write(
      path,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

class PostgresWalletDbBackend extends WalletDbBackend {
  final String url;
  PostgresWalletDbBackend(String this.url);
  PostgresWalletDbBackend._(String this.url);
  static LiftRetVal<PostgresWalletDbBackend> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final url_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final url = url_lifted.value;
    new_offset += url_lifted.bytesRead;
    return LiftRetVal(PostgresWalletDbBackend._(url), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterString.allocationSize(url) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 2);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterString.write(
      url,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

abstract class WalletStore {
  RustBuffer lower();
  int allocationSize();
  int write(Uint8List buf);
}

class FfiConverterWalletStore {
  static WalletStore lift(RustBuffer buffer) {
    return FfiConverterWalletStore.read(buffer.asUint8List()).value;
  }

  static LiftRetVal<WalletStore> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    final subview = Uint8List.view(buf.buffer, buf.offsetInBytes + 4);
    switch (index) {
      case 1:
        final lifted = SqliteWalletStore.read(subview);
        return LiftRetVal<WalletStore>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 2:
        final lifted = PostgresWalletStore.read(subview);
        return LiftRetVal<WalletStore>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 3:
        final lifted = CustomWalletStore.read(subview);
        return LiftRetVal<WalletStore>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static RustBuffer lower(WalletStore value) {
    return value.lower();
  }

  static int allocationSize(WalletStore value) {
    return value.allocationSize();
  }

  static int write(WalletStore value, Uint8List buf) {
    return value.write(buf) - buf.offsetInBytes;
  }
}

class SqliteWalletStore extends WalletStore {
  final String path;
  SqliteWalletStore(String this.path);
  SqliteWalletStore._(String this.path);
  static LiftRetVal<SqliteWalletStore> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final path_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final path = path_lifted.value;
    new_offset += path_lifted.bytesRead;
    return LiftRetVal(SqliteWalletStore._(path), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterString.allocationSize(path) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 1);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterString.write(
      path,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

class PostgresWalletStore extends WalletStore {
  final String url;
  PostgresWalletStore(String this.url);
  PostgresWalletStore._(String this.url);
  static LiftRetVal<PostgresWalletStore> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final url_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final url = url_lifted.value;
    new_offset += url_lifted.bytesRead;
    return LiftRetVal(PostgresWalletStore._(url), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterString.allocationSize(url) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 2);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterString.write(
      url,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

class CustomWalletStore extends WalletStore {
  final WalletDatabase db;
  CustomWalletStore(WalletDatabase this.db);
  CustomWalletStore._(WalletDatabase this.db);
  static LiftRetVal<CustomWalletStore> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final db_lifted = FfiConverterCallbackInterfaceWalletDatabase.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final db = db_lifted.value;
    new_offset += db_lifted.bytesRead;
    return LiftRetVal(CustomWalletStore._(db), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterCallbackInterfaceWalletDatabase.allocationSize(db) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 3);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterCallbackInterfaceWalletDatabase.write(
      db,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

abstract class Witness {
  RustBuffer lower();
  int allocationSize();
  int write(Uint8List buf);
}

class FfiConverterWitness {
  static Witness lift(RustBuffer buffer) {
    return FfiConverterWitness.read(buffer.asUint8List()).value;
  }

  static LiftRetVal<Witness> read(Uint8List buf) {
    final index = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    final subview = Uint8List.view(buf.buffer, buf.offsetInBytes + 4);
    switch (index) {
      case 1:
        final lifted = P2pkWitness.read(subview);
        return LiftRetVal<Witness>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      case 2:
        final lifted = HtlcWitness.read(subview);
        return LiftRetVal<Witness>(
          lifted.value,
          lifted.bytesRead - subview.offsetInBytes + 4,
        );
      default:
        throw UniffiInternalError(
          UniffiInternalError.unexpectedEnumCase,
          "Unable to determine enum variant",
        );
    }
  }

  static RustBuffer lower(Witness value) {
    return value.lower();
  }

  static int allocationSize(Witness value) {
    return value.allocationSize();
  }

  static int write(Witness value, Uint8List buf) {
    return value.write(buf) - buf.offsetInBytes;
  }
}

class P2pkWitness extends Witness {
  final List<String> signatures;
  P2pkWitness(List<String> this.signatures);
  P2pkWitness._(List<String> this.signatures);
  static LiftRetVal<P2pkWitness> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final signatures_lifted = FfiConverterSequenceString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final signatures = signatures_lifted.value;
    new_offset += signatures_lifted.bytesRead;
    return LiftRetVal(P2pkWitness._(signatures), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterSequenceString.allocationSize(signatures) + 4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 1);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterSequenceString.write(
      signatures,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

class HtlcWitness extends Witness {
  final String preimage;
  final List<String>? signatures;
  HtlcWitness({
    required String this.preimage,
    required List<String>? this.signatures,
  });
  HtlcWitness._(String this.preimage, List<String>? this.signatures);
  static LiftRetVal<HtlcWitness> read(Uint8List buf) {
    int new_offset = buf.offsetInBytes;
    final preimage_lifted = FfiConverterString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final preimage = preimage_lifted.value;
    new_offset += preimage_lifted.bytesRead;
    final signatures_lifted = FfiConverterOptionalSequenceString.read(
      Uint8List.view(buf.buffer, new_offset),
    );
    final signatures = signatures_lifted.value;
    new_offset += signatures_lifted.bytesRead;
    return LiftRetVal(HtlcWitness._(preimage, signatures), new_offset);
  }

  @override
  RustBuffer lower() {
    final buf = Uint8List(allocationSize());
    write(buf);
    return toRustBuffer(buf);
  }

  @override
  int allocationSize() {
    return FfiConverterString.allocationSize(preimage) +
        FfiConverterOptionalSequenceString.allocationSize(signatures) +
        4;
  }

  @override
  int write(Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, 2);
    int new_offset = buf.offsetInBytes + 4;
    new_offset += FfiConverterString.write(
      preimage,
      Uint8List.view(buf.buffer, new_offset),
    );
    new_offset += FfiConverterOptionalSequenceString.write(
      signatures,
      Uint8List.view(buf.buffer, new_offset),
    );
    return new_offset;
  }
}

abstract class ActiveSubscriptionInterface {
  String id();
  Future<NotificationPayload> recv();
  Future<NotificationPayload?> tryRecv();
}

final _ActiveSubscriptionFinalizer = Finalizer<Pointer<Void>>((ptr) {
  rustCall((status) => uniffi_cdk_ffi_fn_free_activesubscription(ptr, status));
});

class ActiveSubscription implements ActiveSubscriptionInterface {
  late final Pointer<Void> _ptr;
  ActiveSubscription._(this._ptr) {
    _ActiveSubscriptionFinalizer.attach(this, _ptr, detach: this);
  }
  factory ActiveSubscription.lift(Pointer<Void> ptr) {
    return ActiveSubscription._(ptr);
  }
  static Pointer<Void> lower(ActiveSubscription value) {
    return value.uniffiClonePointer();
  }

  Pointer<Void> uniffiClonePointer() {
    return rustCall(
      (status) => uniffi_cdk_ffi_fn_clone_activesubscription(_ptr, status),
    );
  }

  static int allocationSize(ActiveSubscription value) {
    return 8;
  }

  static LiftRetVal<ActiveSubscription> read(Uint8List buf) {
    final handle = buf.buffer.asByteData(buf.offsetInBytes).getInt64(0);
    final pointer = Pointer<Void>.fromAddress(handle);
    return LiftRetVal(ActiveSubscription.lift(pointer), 8);
  }

  static int write(ActiveSubscription value, Uint8List buf) {
    final handle = lower(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt64(0, handle.address);
    return 8;
  }

  void dispose() {
    _ActiveSubscriptionFinalizer.detach(this);
    rustCall(
      (status) => uniffi_cdk_ffi_fn_free_activesubscription(_ptr, status),
    );
  }

  String id() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_activesubscription_id(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterString.lift,
      null,
    );
  }

  Future<NotificationPayload> recv() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_activesubscription_recv(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterNotificationPayload.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<NotificationPayload?> tryRecv() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_activesubscription_try_recv(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalNotificationPayload.lift,
      ffiExceptionErrorHandler,
    );
  }
}

abstract class NostrWaitInfoInterface {
  String pubkey();
  List<String> relays();
}

final _NostrWaitInfoFinalizer = Finalizer<Pointer<Void>>((ptr) {
  rustCall((status) => uniffi_cdk_ffi_fn_free_nostrwaitinfo(ptr, status));
});

class NostrWaitInfo implements NostrWaitInfoInterface {
  late final Pointer<Void> _ptr;
  NostrWaitInfo._(this._ptr) {
    _NostrWaitInfoFinalizer.attach(this, _ptr, detach: this);
  }
  factory NostrWaitInfo.lift(Pointer<Void> ptr) {
    return NostrWaitInfo._(ptr);
  }
  static Pointer<Void> lower(NostrWaitInfo value) {
    return value.uniffiClonePointer();
  }

  Pointer<Void> uniffiClonePointer() {
    return rustCall(
      (status) => uniffi_cdk_ffi_fn_clone_nostrwaitinfo(_ptr, status),
    );
  }

  static int allocationSize(NostrWaitInfo value) {
    return 8;
  }

  static LiftRetVal<NostrWaitInfo> read(Uint8List buf) {
    final handle = buf.buffer.asByteData(buf.offsetInBytes).getInt64(0);
    final pointer = Pointer<Void>.fromAddress(handle);
    return LiftRetVal(NostrWaitInfo.lift(pointer), 8);
  }

  static int write(NostrWaitInfo value, Uint8List buf) {
    final handle = lower(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt64(0, handle.address);
    return 8;
  }

  void dispose() {
    _NostrWaitInfoFinalizer.detach(this);
    rustCall((status) => uniffi_cdk_ffi_fn_free_nostrwaitinfo(_ptr, status));
  }

  String pubkey() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_nostrwaitinfo_pubkey(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterString.lift,
      null,
    );
  }

  List<String> relays() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_nostrwaitinfo_relays(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterSequenceString.lift,
      null,
    );
  }
}

abstract class NpubCashClientInterface {
  Future<List<NpubCashQuote>> getQuotes({required int? since});
  Future<NpubCashUserResponse> setMintUrl({required String mintUrl});
}

final _NpubCashClientFinalizer = Finalizer<Pointer<Void>>((ptr) {
  rustCall((status) => uniffi_cdk_ffi_fn_free_npubcashclient(ptr, status));
});

class NpubCashClient implements NpubCashClientInterface {
  late final Pointer<Void> _ptr;
  NpubCashClient._(this._ptr) {
    _NpubCashClientFinalizer.attach(this, _ptr, detach: this);
  }
  NpubCashClient({required String baseUrl, required String nostrSecretKey})
    : _ptr = rustCall(
        (status) => uniffi_cdk_ffi_fn_constructor_npubcashclient_new(
          FfiConverterString.lower(baseUrl),
          FfiConverterString.lower(nostrSecretKey),
          status,
        ),
        ffiExceptionErrorHandler,
      ) {
    _NpubCashClientFinalizer.attach(this, _ptr, detach: this);
  }
  factory NpubCashClient.lift(Pointer<Void> ptr) {
    return NpubCashClient._(ptr);
  }
  static Pointer<Void> lower(NpubCashClient value) {
    return value.uniffiClonePointer();
  }

  Pointer<Void> uniffiClonePointer() {
    return rustCall(
      (status) => uniffi_cdk_ffi_fn_clone_npubcashclient(_ptr, status),
    );
  }

  static int allocationSize(NpubCashClient value) {
    return 8;
  }

  static LiftRetVal<NpubCashClient> read(Uint8List buf) {
    final handle = buf.buffer.asByteData(buf.offsetInBytes).getInt64(0);
    final pointer = Pointer<Void>.fromAddress(handle);
    return LiftRetVal(NpubCashClient.lift(pointer), 8);
  }

  static int write(NpubCashClient value, Uint8List buf) {
    final handle = lower(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt64(0, handle.address);
    return 8;
  }

  void dispose() {
    _NpubCashClientFinalizer.detach(this);
    rustCall((status) => uniffi_cdk_ffi_fn_free_npubcashclient(_ptr, status));
  }

  Future<List<NpubCashQuote>> getQuotes({required int? since}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_npubcashclient_get_quotes(
        uniffiClonePointer(),
        FfiConverterOptionalUInt64.lower(since),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceNpubCashQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<NpubCashUserResponse> setMintUrl({required String mintUrl}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_npubcashclient_set_mint_url(
        uniffiClonePointer(),
        FfiConverterString.lower(mintUrl),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterNpubCashUserResponse.lift,
      ffiExceptionErrorHandler,
    );
  }
}

abstract class PaymentRequestInterface {
  Amount? amount();
  String? description();
  List<String> mints();
  String? paymentId();
  bool? singleUse();
  String toStringEncoded();
  List<Transport> transports();
  CurrencyUnit? unit();
}

final _PaymentRequestFinalizer = Finalizer<Pointer<Void>>((ptr) {
  rustCall((status) => uniffi_cdk_ffi_fn_free_paymentrequest(ptr, status));
});

class PaymentRequest implements PaymentRequestInterface {
  late final Pointer<Void> _ptr;
  PaymentRequest._(this._ptr) {
    _PaymentRequestFinalizer.attach(this, _ptr, detach: this);
  }
  PaymentRequest.fromString({required String encoded})
    : _ptr = rustCall(
        (status) => uniffi_cdk_ffi_fn_constructor_paymentrequest_from_string(
          FfiConverterString.lower(encoded),
          status,
        ),
        ffiExceptionErrorHandler,
      ) {
    _PaymentRequestFinalizer.attach(this, _ptr, detach: this);
  }
  factory PaymentRequest.lift(Pointer<Void> ptr) {
    return PaymentRequest._(ptr);
  }
  static Pointer<Void> lower(PaymentRequest value) {
    return value.uniffiClonePointer();
  }

  Pointer<Void> uniffiClonePointer() {
    return rustCall(
      (status) => uniffi_cdk_ffi_fn_clone_paymentrequest(_ptr, status),
    );
  }

  static int allocationSize(PaymentRequest value) {
    return 8;
  }

  static LiftRetVal<PaymentRequest> read(Uint8List buf) {
    final handle = buf.buffer.asByteData(buf.offsetInBytes).getInt64(0);
    final pointer = Pointer<Void>.fromAddress(handle);
    return LiftRetVal(PaymentRequest.lift(pointer), 8);
  }

  static int write(PaymentRequest value, Uint8List buf) {
    final handle = lower(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt64(0, handle.address);
    return 8;
  }

  void dispose() {
    _PaymentRequestFinalizer.detach(this);
    rustCall((status) => uniffi_cdk_ffi_fn_free_paymentrequest(_ptr, status));
  }

  Amount? amount() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_paymentrequest_amount(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterOptionalAmount.lift,
      null,
    );
  }

  String? description() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_paymentrequest_description(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterOptionalString.lift,
      null,
    );
  }

  List<String> mints() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_paymentrequest_mints(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterSequenceString.lift,
      null,
    );
  }

  String? paymentId() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_paymentrequest_payment_id(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterOptionalString.lift,
      null,
    );
  }

  bool? singleUse() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_paymentrequest_single_use(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterOptionalBool.lift,
      null,
    );
  }

  String toStringEncoded() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_paymentrequest_to_string_encoded(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterString.lift,
      null,
    );
  }

  List<Transport> transports() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_paymentrequest_transports(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterSequenceTransport.lift,
      null,
    );
  }

  CurrencyUnit? unit() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_paymentrequest_unit(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterOptionalCurrencyUnit.lift,
      null,
    );
  }
}

abstract class PaymentRequestPayloadInterface {
  String? id();
  String? memo();
  MintUrl mint();
  List<Proof> proofs();
  CurrencyUnit unit();
}

final _PaymentRequestPayloadFinalizer = Finalizer<Pointer<Void>>((ptr) {
  rustCall(
    (status) => uniffi_cdk_ffi_fn_free_paymentrequestpayload(ptr, status),
  );
});

class PaymentRequestPayload implements PaymentRequestPayloadInterface {
  late final Pointer<Void> _ptr;
  PaymentRequestPayload._(this._ptr) {
    _PaymentRequestPayloadFinalizer.attach(this, _ptr, detach: this);
  }
  PaymentRequestPayload.fromString({required String json})
    : _ptr = rustCall(
        (status) =>
            uniffi_cdk_ffi_fn_constructor_paymentrequestpayload_from_string(
              FfiConverterString.lower(json),
              status,
            ),
        ffiExceptionErrorHandler,
      ) {
    _PaymentRequestPayloadFinalizer.attach(this, _ptr, detach: this);
  }
  factory PaymentRequestPayload.lift(Pointer<Void> ptr) {
    return PaymentRequestPayload._(ptr);
  }
  static Pointer<Void> lower(PaymentRequestPayload value) {
    return value.uniffiClonePointer();
  }

  Pointer<Void> uniffiClonePointer() {
    return rustCall(
      (status) => uniffi_cdk_ffi_fn_clone_paymentrequestpayload(_ptr, status),
    );
  }

  static int allocationSize(PaymentRequestPayload value) {
    return 8;
  }

  static LiftRetVal<PaymentRequestPayload> read(Uint8List buf) {
    final handle = buf.buffer.asByteData(buf.offsetInBytes).getInt64(0);
    final pointer = Pointer<Void>.fromAddress(handle);
    return LiftRetVal(PaymentRequestPayload.lift(pointer), 8);
  }

  static int write(PaymentRequestPayload value, Uint8List buf) {
    final handle = lower(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt64(0, handle.address);
    return 8;
  }

  void dispose() {
    _PaymentRequestPayloadFinalizer.detach(this);
    rustCall(
      (status) => uniffi_cdk_ffi_fn_free_paymentrequestpayload(_ptr, status),
    );
  }

  String? id() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_paymentrequestpayload_id(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterOptionalString.lift,
      null,
    );
  }

  String? memo() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_paymentrequestpayload_memo(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterOptionalString.lift,
      null,
    );
  }

  MintUrl mint() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_paymentrequestpayload_mint(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterMintUrl.lift,
      null,
    );
  }

  List<Proof> proofs() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_paymentrequestpayload_proofs(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterSequenceProof.lift,
      null,
    );
  }

  CurrencyUnit unit() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_paymentrequestpayload_unit(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterCurrencyUnit.lift,
      null,
    );
  }
}

abstract class PreparedMeltInterface {
  Amount amount();
  Future<void> cancel();
  Amount changeAmountWithoutSwap();
  Future<FinalizedMelt> confirm();
  Future<FinalizedMelt> confirmWithOptions({
    required MeltConfirmOptions options,
  });
  Amount feeReserve();
  Amount feeSavingsWithoutSwap();
  Amount inputFee();
  Amount inputFeeWithoutSwap();
  String operationId();
  List<Proof> proofs();
  String quoteId();
  bool requiresSwap();
  Amount swapFee();
  Amount totalFee();
  Amount totalFeeWithSwap();
}

final _PreparedMeltFinalizer = Finalizer<Pointer<Void>>((ptr) {
  rustCall((status) => uniffi_cdk_ffi_fn_free_preparedmelt(ptr, status));
});

class PreparedMelt implements PreparedMeltInterface {
  late final Pointer<Void> _ptr;
  PreparedMelt._(this._ptr) {
    _PreparedMeltFinalizer.attach(this, _ptr, detach: this);
  }
  factory PreparedMelt.lift(Pointer<Void> ptr) {
    return PreparedMelt._(ptr);
  }
  static Pointer<Void> lower(PreparedMelt value) {
    return value.uniffiClonePointer();
  }

  Pointer<Void> uniffiClonePointer() {
    return rustCall(
      (status) => uniffi_cdk_ffi_fn_clone_preparedmelt(_ptr, status),
    );
  }

  static int allocationSize(PreparedMelt value) {
    return 8;
  }

  static LiftRetVal<PreparedMelt> read(Uint8List buf) {
    final handle = buf.buffer.asByteData(buf.offsetInBytes).getInt64(0);
    final pointer = Pointer<Void>.fromAddress(handle);
    return LiftRetVal(PreparedMelt.lift(pointer), 8);
  }

  static int write(PreparedMelt value, Uint8List buf) {
    final handle = lower(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt64(0, handle.address);
    return 8;
  }

  void dispose() {
    _PreparedMeltFinalizer.detach(this);
    rustCall((status) => uniffi_cdk_ffi_fn_free_preparedmelt(_ptr, status));
  }

  Amount amount() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedmelt_amount(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterAmount.lift,
      null,
    );
  }

  Future<void> cancel() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_preparedmelt_cancel(uniffiClonePointer()),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Amount changeAmountWithoutSwap() {
    return rustCallWithLifter(
      (status) =>
          uniffi_cdk_ffi_fn_method_preparedmelt_change_amount_without_swap(
            uniffiClonePointer(),
            status,
          ),
      FfiConverterAmount.lift,
      null,
    );
  }

  Future<FinalizedMelt> confirm() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_preparedmelt_confirm(uniffiClonePointer()),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterFinalizedMelt.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<FinalizedMelt> confirmWithOptions({
    required MeltConfirmOptions options,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_preparedmelt_confirm_with_options(
        uniffiClonePointer(),
        FfiConverterMeltConfirmOptions.lower(options),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterFinalizedMelt.lift,
      ffiExceptionErrorHandler,
    );
  }

  Amount feeReserve() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedmelt_fee_reserve(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterAmount.lift,
      null,
    );
  }

  Amount feeSavingsWithoutSwap() {
    return rustCallWithLifter(
      (status) =>
          uniffi_cdk_ffi_fn_method_preparedmelt_fee_savings_without_swap(
            uniffiClonePointer(),
            status,
          ),
      FfiConverterAmount.lift,
      null,
    );
  }

  Amount inputFee() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedmelt_input_fee(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterAmount.lift,
      null,
    );
  }

  Amount inputFeeWithoutSwap() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedmelt_input_fee_without_swap(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterAmount.lift,
      null,
    );
  }

  String operationId() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedmelt_operation_id(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterString.lift,
      null,
    );
  }

  List<Proof> proofs() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedmelt_proofs(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterSequenceProof.lift,
      null,
    );
  }

  String quoteId() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedmelt_quote_id(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterString.lift,
      null,
    );
  }

  bool requiresSwap() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedmelt_requires_swap(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterBool.lift,
      null,
    );
  }

  Amount swapFee() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedmelt_swap_fee(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterAmount.lift,
      null,
    );
  }

  Amount totalFee() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedmelt_total_fee(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterAmount.lift,
      null,
    );
  }

  Amount totalFeeWithSwap() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedmelt_total_fee_with_swap(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterAmount.lift,
      null,
    );
  }
}

abstract class PreparedSendInterface {
  Amount amount();
  Future<void> cancel();
  Future<Token> confirm({required String? memo});
  Amount fee();
  String operationId();
  List<Proof> proofs();
}

final _PreparedSendFinalizer = Finalizer<Pointer<Void>>((ptr) {
  rustCall((status) => uniffi_cdk_ffi_fn_free_preparedsend(ptr, status));
});

class PreparedSend implements PreparedSendInterface {
  late final Pointer<Void> _ptr;
  PreparedSend._(this._ptr) {
    _PreparedSendFinalizer.attach(this, _ptr, detach: this);
  }
  factory PreparedSend.lift(Pointer<Void> ptr) {
    return PreparedSend._(ptr);
  }
  static Pointer<Void> lower(PreparedSend value) {
    return value.uniffiClonePointer();
  }

  Pointer<Void> uniffiClonePointer() {
    return rustCall(
      (status) => uniffi_cdk_ffi_fn_clone_preparedsend(_ptr, status),
    );
  }

  static int allocationSize(PreparedSend value) {
    return 8;
  }

  static LiftRetVal<PreparedSend> read(Uint8List buf) {
    final handle = buf.buffer.asByteData(buf.offsetInBytes).getInt64(0);
    final pointer = Pointer<Void>.fromAddress(handle);
    return LiftRetVal(PreparedSend.lift(pointer), 8);
  }

  static int write(PreparedSend value, Uint8List buf) {
    final handle = lower(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt64(0, handle.address);
    return 8;
  }

  void dispose() {
    _PreparedSendFinalizer.detach(this);
    rustCall((status) => uniffi_cdk_ffi_fn_free_preparedsend(_ptr, status));
  }

  Amount amount() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedsend_amount(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterAmount.lift,
      null,
    );
  }

  Future<void> cancel() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_preparedsend_cancel(uniffiClonePointer()),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<Token> confirm({required String? memo}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_preparedsend_confirm(
        uniffiClonePointer(),
        FfiConverterOptionalString.lower(memo),
      ),
      ffi_cdk_ffi_rust_future_poll_u64,
      ffi_cdk_ffi_rust_future_complete_u64,
      ffi_cdk_ffi_rust_future_free_u64,
      (ptr) => Token.lift(Pointer<Void>.fromAddress(ptr)),
      ffiExceptionErrorHandler,
    );
  }

  Amount fee() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedsend_fee(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterAmount.lift,
      null,
    );
  }

  String operationId() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedsend_operation_id(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterString.lift,
      null,
    );
  }

  List<Proof> proofs() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_preparedsend_proofs(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterSequenceProof.lift,
      null,
    );
  }
}

abstract class TokenInterface {
  String encode();
  List<String> htlcHashes();
  List<int> locktimes();
  String? memo();
  MintUrl mintUrl();
  List<String> p2pkPubkeys();
  List<String> p2pkRefundPubkeys();
  List<Proof> proofs({required List<KeySetInfo> mintKeysets});
  List<Proof> proofsSimple();
  List<SpendingConditions> spendingConditions();
  Uint8List toRawBytes();
  CurrencyUnit? unit();
  Amount value();
}

final _TokenFinalizer = Finalizer<Pointer<Void>>((ptr) {
  rustCall((status) => uniffi_cdk_ffi_fn_free_token(ptr, status));
});

class Token implements TokenInterface {
  late final Pointer<Void> _ptr;
  Token._(this._ptr) {
    _TokenFinalizer.attach(this, _ptr, detach: this);
  }
  Token.decode({required String encodedToken})
    : _ptr = rustCall(
        (status) => uniffi_cdk_ffi_fn_constructor_token_decode(
          FfiConverterString.lower(encodedToken),
          status,
        ),
        ffiExceptionErrorHandler,
      ) {
    _TokenFinalizer.attach(this, _ptr, detach: this);
  }
  Token.fromRawBytes({required Uint8List bytes})
    : _ptr = rustCall(
        (status) => uniffi_cdk_ffi_fn_constructor_token_from_raw_bytes(
          FfiConverterUint8List.lower(bytes),
          status,
        ),
        ffiExceptionErrorHandler,
      ) {
    _TokenFinalizer.attach(this, _ptr, detach: this);
  }
  Token.fromString({required String encodedToken})
    : _ptr = rustCall(
        (status) => uniffi_cdk_ffi_fn_constructor_token_from_string(
          FfiConverterString.lower(encodedToken),
          status,
        ),
        ffiExceptionErrorHandler,
      ) {
    _TokenFinalizer.attach(this, _ptr, detach: this);
  }
  factory Token.lift(Pointer<Void> ptr) {
    return Token._(ptr);
  }
  static Pointer<Void> lower(Token value) {
    return value.uniffiClonePointer();
  }

  Pointer<Void> uniffiClonePointer() {
    return rustCall((status) => uniffi_cdk_ffi_fn_clone_token(_ptr, status));
  }

  static int allocationSize(Token value) {
    return 8;
  }

  static LiftRetVal<Token> read(Uint8List buf) {
    final handle = buf.buffer.asByteData(buf.offsetInBytes).getInt64(0);
    final pointer = Pointer<Void>.fromAddress(handle);
    return LiftRetVal(Token.lift(pointer), 8);
  }

  static int write(Token value, Uint8List buf) {
    final handle = lower(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt64(0, handle.address);
    return 8;
  }

  void dispose() {
    _TokenFinalizer.detach(this);
    rustCall((status) => uniffi_cdk_ffi_fn_free_token(_ptr, status));
  }

  String encode() {
    return rustCallWithLifter(
      (status) =>
          uniffi_cdk_ffi_fn_method_token_encode(uniffiClonePointer(), status),
      FfiConverterString.lift,
      null,
    );
  }

  List<String> htlcHashes() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_token_htlc_hashes(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterSequenceString.lift,
      null,
    );
  }

  List<int> locktimes() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_token_locktimes(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterSequenceUInt64.lift,
      null,
    );
  }

  String? memo() {
    return rustCallWithLifter(
      (status) =>
          uniffi_cdk_ffi_fn_method_token_memo(uniffiClonePointer(), status),
      FfiConverterOptionalString.lift,
      null,
    );
  }

  MintUrl mintUrl() {
    return rustCallWithLifter(
      (status) =>
          uniffi_cdk_ffi_fn_method_token_mint_url(uniffiClonePointer(), status),
      FfiConverterMintUrl.lift,
      ffiExceptionErrorHandler,
    );
  }

  List<String> p2pkPubkeys() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_token_p2pk_pubkeys(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterSequenceString.lift,
      null,
    );
  }

  List<String> p2pkRefundPubkeys() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_token_p2pk_refund_pubkeys(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterSequenceString.lift,
      null,
    );
  }

  List<Proof> proofs({required List<KeySetInfo> mintKeysets}) {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_token_proofs(
        uniffiClonePointer(),
        FfiConverterSequenceKeySetInfo.lower(mintKeysets),
        status,
      ),
      FfiConverterSequenceProof.lift,
      ffiExceptionErrorHandler,
    );
  }

  List<Proof> proofsSimple() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_token_proofs_simple(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterSequenceProof.lift,
      ffiExceptionErrorHandler,
    );
  }

  List<SpendingConditions> spendingConditions() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_token_spending_conditions(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterSequenceSpendingConditions.lift,
      null,
    );
  }

  Uint8List toRawBytes() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_token_to_raw_bytes(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterUint8List.lift,
      ffiExceptionErrorHandler,
    );
  }

  CurrencyUnit? unit() {
    return rustCallWithLifter(
      (status) =>
          uniffi_cdk_ffi_fn_method_token_unit(uniffiClonePointer(), status),
      FfiConverterOptionalCurrencyUnit.lift,
      null,
    );
  }

  Amount value() {
    return rustCallWithLifter(
      (status) =>
          uniffi_cdk_ffi_fn_method_token_value(uniffiClonePointer(), status),
      FfiConverterAmount.lift,
      ffiExceptionErrorHandler,
    );
  }
}

abstract class WalletInterface {
  Future<Amount> calculateFee({
    required int proofCount,
    required String keysetId,
  });
  Future<Amount> checkAllPendingProofs();
  Future<MintQuote> checkMintQuote({required String quoteId});
  Future<List<bool>> checkProofsSpent({required List<Proof> proofs});
  Future<bool> checkSendStatus({required String operationId});
  Future<MintInfo?> fetchMintInfo();
  Future<MintQuote> fetchMintQuote({
    required String quoteId,
    required PaymentMethod? paymentMethod,
  });
  Future<KeySetInfo> getActiveKeyset();
  Future<int> getKeysetFeesById({required String keysetId});
  Future<List<String>> getPendingSends();
  Future<List<Proof>> getProofsByStates({required List<ProofState> states});
  Future<List<Proof>> getProofsForTransaction({required TransactionId id});
  Future<Transaction?> getTransaction({required TransactionId id});
  Future<List<AuthProof>> getUnspentAuthProofs();
  Future<List<Transaction>> listTransactions({
    required TransactionDirection? direction,
  });
  Future<MintInfo> loadMintInfo();
  Future<MeltQuote> meltBip353Quote({
    required String bip353Address,
    required Amount amountMsat,
  });
  Future<MeltQuote> meltHumanReadable({
    required String address,
    required Amount amountMsat,
  });
  Future<MeltQuote> meltLightningAddressQuote({
    required String lightningAddress,
    required Amount amountMsat,
  });
  Future<MeltQuote> meltQuote({
    required PaymentMethod method,
    required String request,
    required MeltOptions? options,
    required String? extra,
  });
  Future<List<Proof>> mint({
    required String quoteId,
    required SplitTarget amountSplitTarget,
    required SpendingConditions? spendingConditions,
  });
  Future<List<Proof>> mintBlindAuth({required Amount amount});
  Future<MintQuote> mintQuote({
    required PaymentMethod paymentMethod,
    required Amount? amount,
    required String? description,
    required String? extra,
  });
  Future<List<Proof>> mintUnified({
    required String quoteId,
    required SplitTarget amountSplitTarget,
    required SpendingConditions? spendingConditions,
  });
  MintUrl mintUrl();
  Future<void> payRequest({
    required PaymentRequest paymentRequest,
    required Amount? customAmount,
  });
  Future<PreparedMelt> prepareMelt({required String quoteId});
  Future<PreparedMelt> prepareMeltProofs({
    required String quoteId,
    required List<Proof> proofs,
  });
  Future<PreparedSend> prepareSend({
    required Amount amount,
    required SendOptions options,
  });
  Future<Amount> receive({
    required Token token,
    required ReceiveOptions options,
  });
  Future<Amount> receiveProofs({
    required List<Proof> proofs,
    required ReceiveOptions options,
    required String? memo,
    required String? token,
  });
  Future<void> refreshAccessToken();
  Future<List<KeySetInfo>> refreshKeysets();
  Future<Restored> restore();
  Future<void> revertTransaction({required TransactionId id});
  Future<Amount> revokeSend({required String operationId});
  Future<void> setCat({required String cat});
  void setMetadataCacheTtl({required int? ttlSecs});
  Future<void> setRefreshToken({required String refreshToken});
  Future<ActiveSubscription> subscribe({required SubscribeParams params});
  Future<List<Proof>?> swap({
    required Amount? amount,
    required SplitTarget amountSplitTarget,
    required List<Proof> inputProofs,
    required SpendingConditions? spendingConditions,
    required bool includeFees,
  });
  Future<Amount> totalBalance();
  Future<Amount> totalPendingBalance();
  Future<Amount> totalReservedBalance();
  CurrencyUnit unit();
  Future<void> verifyTokenDleq({required Token token});
}

final _WalletFinalizer = Finalizer<Pointer<Void>>((ptr) {
  rustCall((status) => uniffi_cdk_ffi_fn_free_wallet(ptr, status));
});

class Wallet implements WalletInterface {
  late final Pointer<Void> _ptr;
  Wallet._(this._ptr) {
    _WalletFinalizer.attach(this, _ptr, detach: this);
  }
  Wallet({
    required String mintUrl,
    required CurrencyUnit unit,
    required String mnemonic,
    required WalletStore store,
    required WalletConfig config,
  }) : _ptr = rustCall(
         (status) => uniffi_cdk_ffi_fn_constructor_wallet_new(
           FfiConverterString.lower(mintUrl),
           FfiConverterCurrencyUnit.lower(unit),
           FfiConverterString.lower(mnemonic),
           FfiConverterWalletStore.lower(store),
           FfiConverterWalletConfig.lower(config),
           status,
         ),
         ffiExceptionErrorHandler,
       ) {
    _WalletFinalizer.attach(this, _ptr, detach: this);
  }
  factory Wallet.lift(Pointer<Void> ptr) {
    return Wallet._(ptr);
  }
  static Pointer<Void> lower(Wallet value) {
    return value.uniffiClonePointer();
  }

  Pointer<Void> uniffiClonePointer() {
    return rustCall((status) => uniffi_cdk_ffi_fn_clone_wallet(_ptr, status));
  }

  static int allocationSize(Wallet value) {
    return 8;
  }

  static LiftRetVal<Wallet> read(Uint8List buf) {
    final handle = buf.buffer.asByteData(buf.offsetInBytes).getInt64(0);
    final pointer = Pointer<Void>.fromAddress(handle);
    return LiftRetVal(Wallet.lift(pointer), 8);
  }

  static int write(Wallet value, Uint8List buf) {
    final handle = lower(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt64(0, handle.address);
    return 8;
  }

  void dispose() {
    _WalletFinalizer.detach(this);
    rustCall((status) => uniffi_cdk_ffi_fn_free_wallet(_ptr, status));
  }

  Future<Amount> calculateFee({
    required int proofCount,
    required String keysetId,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_calculate_fee(
        uniffiClonePointer(),
        FfiConverterUInt32.lower(proofCount),
        FfiConverterString.lower(keysetId),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterAmount.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Amount> checkAllPendingProofs() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_check_all_pending_proofs(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterAmount.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MintQuote> checkMintQuote({required String quoteId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_check_mint_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterMintQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<bool>> checkProofsSpent({required List<Proof> proofs}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_check_proofs_spent(
        uniffiClonePointer(),
        FfiConverterSequenceProof.lower(proofs),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceBool.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<bool> checkSendStatus({required String operationId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_check_send_status(
        uniffiClonePointer(),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_i8,
      ffi_cdk_ffi_rust_future_complete_i8,
      ffi_cdk_ffi_rust_future_free_i8,
      FfiConverterBool.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MintInfo?> fetchMintInfo() {
    return uniffiRustCallAsync(
      () =>
          uniffi_cdk_ffi_fn_method_wallet_fetch_mint_info(uniffiClonePointer()),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalMintInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MintQuote> fetchMintQuote({
    required String quoteId,
    required PaymentMethod? paymentMethod,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_fetch_mint_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
        FfiConverterOptionalPaymentMethod.lower(paymentMethod),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterMintQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<KeySetInfo> getActiveKeyset() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_get_active_keyset(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterKeySetInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<int> getKeysetFeesById({required String keysetId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_get_keyset_fees_by_id(
        uniffiClonePointer(),
        FfiConverterString.lower(keysetId),
      ),
      ffi_cdk_ffi_rust_future_poll_u64,
      ffi_cdk_ffi_rust_future_complete_u64,
      ffi_cdk_ffi_rust_future_free_u64,
      FfiConverterUInt64.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<String>> getPendingSends() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_get_pending_sends(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceString.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<Proof>> getProofsByStates({required List<ProofState> states}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_get_proofs_by_states(
        uniffiClonePointer(),
        FfiConverterSequenceProofState.lower(states),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceProof.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<Proof>> getProofsForTransaction({required TransactionId id}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_get_proofs_for_transaction(
        uniffiClonePointer(),
        FfiConverterTransactionId.lower(id),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceProof.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Transaction?> getTransaction({required TransactionId id}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_get_transaction(
        uniffiClonePointer(),
        FfiConverterTransactionId.lower(id),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalTransaction.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<AuthProof>> getUnspentAuthProofs() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_get_unspent_auth_proofs(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceAuthProof.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<Transaction>> listTransactions({
    required TransactionDirection? direction,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_list_transactions(
        uniffiClonePointer(),
        FfiConverterOptionalTransactionDirection.lower(direction),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceTransaction.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MintInfo> loadMintInfo() {
    return uniffiRustCallAsync(
      () =>
          uniffi_cdk_ffi_fn_method_wallet_load_mint_info(uniffiClonePointer()),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterMintInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MeltQuote> meltBip353Quote({
    required String bip353Address,
    required Amount amountMsat,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_melt_bip353_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(bip353Address),
        FfiConverterAmount.lower(amountMsat),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterMeltQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MeltQuote> meltHumanReadable({
    required String address,
    required Amount amountMsat,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_melt_human_readable(
        uniffiClonePointer(),
        FfiConverterString.lower(address),
        FfiConverterAmount.lower(amountMsat),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterMeltQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MeltQuote> meltLightningAddressQuote({
    required String lightningAddress,
    required Amount amountMsat,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_melt_lightning_address_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(lightningAddress),
        FfiConverterAmount.lower(amountMsat),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterMeltQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MeltQuote> meltQuote({
    required PaymentMethod method,
    required String request,
    required MeltOptions? options,
    required String? extra,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_melt_quote(
        uniffiClonePointer(),
        FfiConverterPaymentMethod.lower(method),
        FfiConverterString.lower(request),
        FfiConverterOptionalMeltOptions.lower(options),
        FfiConverterOptionalString.lower(extra),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterMeltQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<Proof>> mint({
    required String quoteId,
    required SplitTarget amountSplitTarget,
    required SpendingConditions? spendingConditions,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_mint(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
        FfiConverterSplitTarget.lower(amountSplitTarget),
        FfiConverterOptionalSpendingConditions.lower(spendingConditions),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceProof.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<Proof>> mintBlindAuth({required Amount amount}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_mint_blind_auth(
        uniffiClonePointer(),
        FfiConverterAmount.lower(amount),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceProof.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MintQuote> mintQuote({
    required PaymentMethod paymentMethod,
    required Amount? amount,
    required String? description,
    required String? extra,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_mint_quote(
        uniffiClonePointer(),
        FfiConverterPaymentMethod.lower(paymentMethod),
        FfiConverterOptionalAmount.lower(amount),
        FfiConverterOptionalString.lower(description),
        FfiConverterOptionalString.lower(extra),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterMintQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<Proof>> mintUnified({
    required String quoteId,
    required SplitTarget amountSplitTarget,
    required SpendingConditions? spendingConditions,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_mint_unified(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
        FfiConverterSplitTarget.lower(amountSplitTarget),
        FfiConverterOptionalSpendingConditions.lower(spendingConditions),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceProof.lift,
      ffiExceptionErrorHandler,
    );
  }

  MintUrl mintUrl() {
    return rustCallWithLifter(
      (status) => uniffi_cdk_ffi_fn_method_wallet_mint_url(
        uniffiClonePointer(),
        status,
      ),
      FfiConverterMintUrl.lift,
      null,
    );
  }

  Future<void> payRequest({
    required PaymentRequest paymentRequest,
    required Amount? customAmount,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_pay_request(
        uniffiClonePointer(),
        PaymentRequest.lower(paymentRequest),
        FfiConverterOptionalAmount.lower(customAmount),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<PreparedMelt> prepareMelt({required String quoteId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_prepare_melt(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
      ),
      ffi_cdk_ffi_rust_future_poll_u64,
      ffi_cdk_ffi_rust_future_complete_u64,
      ffi_cdk_ffi_rust_future_free_u64,
      (ptr) => PreparedMelt.lift(Pointer<Void>.fromAddress(ptr)),
      ffiExceptionErrorHandler,
    );
  }

  Future<PreparedMelt> prepareMeltProofs({
    required String quoteId,
    required List<Proof> proofs,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_prepare_melt_proofs(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
        FfiConverterSequenceProof.lower(proofs),
      ),
      ffi_cdk_ffi_rust_future_poll_u64,
      ffi_cdk_ffi_rust_future_complete_u64,
      ffi_cdk_ffi_rust_future_free_u64,
      (ptr) => PreparedMelt.lift(Pointer<Void>.fromAddress(ptr)),
      ffiExceptionErrorHandler,
    );
  }

  Future<PreparedSend> prepareSend({
    required Amount amount,
    required SendOptions options,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_prepare_send(
        uniffiClonePointer(),
        FfiConverterAmount.lower(amount),
        FfiConverterSendOptions.lower(options),
      ),
      ffi_cdk_ffi_rust_future_poll_u64,
      ffi_cdk_ffi_rust_future_complete_u64,
      ffi_cdk_ffi_rust_future_free_u64,
      (ptr) => PreparedSend.lift(Pointer<Void>.fromAddress(ptr)),
      ffiExceptionErrorHandler,
    );
  }

  Future<Amount> receive({
    required Token token,
    required ReceiveOptions options,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_receive(
        uniffiClonePointer(),
        Token.lower(token),
        FfiConverterReceiveOptions.lower(options),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterAmount.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Amount> receiveProofs({
    required List<Proof> proofs,
    required ReceiveOptions options,
    required String? memo,
    required String? token,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_receive_proofs(
        uniffiClonePointer(),
        FfiConverterSequenceProof.lower(proofs),
        FfiConverterReceiveOptions.lower(options),
        FfiConverterOptionalString.lower(memo),
        FfiConverterOptionalString.lower(token),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterAmount.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<void> refreshAccessToken() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_refresh_access_token(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<List<KeySetInfo>> refreshKeysets() {
    return uniffiRustCallAsync(
      () =>
          uniffi_cdk_ffi_fn_method_wallet_refresh_keysets(uniffiClonePointer()),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceKeySetInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Restored> restore() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_restore(uniffiClonePointer()),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterRestored.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<void> revertTransaction({required TransactionId id}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_revert_transaction(
        uniffiClonePointer(),
        FfiConverterTransactionId.lower(id),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<Amount> revokeSend({required String operationId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_revoke_send(
        uniffiClonePointer(),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterAmount.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<void> setCat({required String cat}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_set_cat(
        uniffiClonePointer(),
        FfiConverterString.lower(cat),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  void setMetadataCacheTtl({required int? ttlSecs}) {
    return rustCall((status) {
      uniffi_cdk_ffi_fn_method_wallet_set_metadata_cache_ttl(
        uniffiClonePointer(),
        FfiConverterOptionalUInt64.lower(ttlSecs),
        status,
      );
    }, null);
  }

  Future<void> setRefreshToken({required String refreshToken}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_set_refresh_token(
        uniffiClonePointer(),
        FfiConverterString.lower(refreshToken),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<ActiveSubscription> subscribe({required SubscribeParams params}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_subscribe(
        uniffiClonePointer(),
        FfiConverterSubscribeParams.lower(params),
      ),
      ffi_cdk_ffi_rust_future_poll_u64,
      ffi_cdk_ffi_rust_future_complete_u64,
      ffi_cdk_ffi_rust_future_free_u64,
      (ptr) => ActiveSubscription.lift(Pointer<Void>.fromAddress(ptr)),
      ffiExceptionErrorHandler,
    );
  }

  Future<List<Proof>?> swap({
    required Amount? amount,
    required SplitTarget amountSplitTarget,
    required List<Proof> inputProofs,
    required SpendingConditions? spendingConditions,
    required bool includeFees,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_swap(
        uniffiClonePointer(),
        FfiConverterOptionalAmount.lower(amount),
        FfiConverterSplitTarget.lower(amountSplitTarget),
        FfiConverterSequenceProof.lower(inputProofs),
        FfiConverterOptionalSpendingConditions.lower(spendingConditions),
        FfiConverterBool.lower(includeFees),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalSequenceProof.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Amount> totalBalance() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_total_balance(uniffiClonePointer()),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterAmount.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Amount> totalPendingBalance() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_total_pending_balance(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterAmount.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Amount> totalReservedBalance() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_total_reserved_balance(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterAmount.lift,
      ffiExceptionErrorHandler,
    );
  }

  CurrencyUnit unit() {
    return rustCallWithLifter(
      (status) =>
          uniffi_cdk_ffi_fn_method_wallet_unit(uniffiClonePointer(), status),
      FfiConverterCurrencyUnit.lift,
      null,
    );
  }

  Future<void> verifyTokenDleq({required Token token}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_wallet_verify_token_dleq(
        uniffiClonePointer(),
        Token.lower(token),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }
}

abstract class WalletDatabase {
  Future<MintInfo?> getMint(MintUrl mintUrl);
  Future<Map<MintUrl, MintInfo?>> getMints();
  Future<List<KeySetInfo>?> getMintKeysets(MintUrl mintUrl);
  Future<KeySetInfo?> getKeysetById(Id keysetId);
  Future<MintQuote?> getMintQuote(String quoteId);
  Future<List<MintQuote>> getMintQuotes();
  Future<List<MintQuote>> getUnissuedMintQuotes();
  Future<MeltQuote?> getMeltQuote(String quoteId);
  Future<List<MeltQuote>> getMeltQuotes();
  Future<Keys?> getKeys(Id id);
  Future<List<ProofInfo>> getProofs(
    MintUrl? mintUrl,
    CurrencyUnit? unit,
    List<ProofState>? state,
    List<SpendingConditions>? spendingConditions,
  );
  Future<List<ProofInfo>> getProofsByYs(List<PublicKey> ys);
  Future<int> getBalance(
    MintUrl? mintUrl,
    CurrencyUnit? unit,
    List<ProofState>? state,
  );
  Future<Transaction?> getTransaction(TransactionId transactionId);
  Future<List<Transaction>> listTransactions(
    MintUrl? mintUrl,
    TransactionDirection? direction,
    CurrencyUnit? unit,
  );
  Future<Uint8List?> kvRead(
    String primaryNamespace,
    String secondaryNamespace,
    String key,
  );
  Future<List<String>> kvList(
    String primaryNamespace,
    String secondaryNamespace,
  );
  Future<void> kvWrite(
    String primaryNamespace,
    String secondaryNamespace,
    String key,
    Uint8List value,
  );
  Future<void> kvRemove(
    String primaryNamespace,
    String secondaryNamespace,
    String key,
  );
  Future<void> updateProofs(List<ProofInfo> added, List<PublicKey> removedYs);
  Future<void> updateProofsState(List<PublicKey> ys, ProofState state);
  Future<void> addTransaction(Transaction transaction);
  Future<void> removeTransaction(TransactionId transactionId);
  Future<void> updateMintUrl(MintUrl oldMintUrl, MintUrl newMintUrl);
  Future<int> incrementKeysetCounter(Id keysetId, int count);
  Future<void> addMint(MintUrl mintUrl, MintInfo? mintInfo);
  Future<void> removeMint(MintUrl mintUrl);
  Future<void> addMintKeysets(MintUrl mintUrl, List<KeySetInfo> keysets);
  Future<void> addMintQuote(MintQuote quote);
  Future<void> removeMintQuote(String quoteId);
  Future<void> addMeltQuote(MeltQuote quote);
  Future<void> removeMeltQuote(String quoteId);
  Future<void> addKeys(KeySet keyset);
  Future<void> removeKeys(Id id);
  Future<void> addSaga(String sagaJson);
  Future<String?> getSaga(String id);
  Future<bool> updateSaga(String sagaJson);
  Future<void> deleteSaga(String id);
  Future<List<String>> getIncompleteSagas();
  Future<void> reserveProofs(List<PublicKey> ys, String operationId);
  Future<void> releaseProofs(String operationId);
  Future<List<ProofInfo>> getReservedProofs(String operationId);
  Future<void> reserveMeltQuote(String quoteId, String operationId);
  Future<void> releaseMeltQuote(String operationId);
  Future<void> reserveMintQuote(String quoteId, String operationId);
  Future<void> releaseMintQuote(String operationId);
}


/// Proxy for Rust-created WalletDatabase objects.
/// Holds a raw Rust Arc pointer; all trait methods throw because
/// Dart never calls them — the object is only lowered back to Rust.
class _RustOwnedWalletDatabase implements WalletDatabase {
  final Pointer<Void> _ptr;
  _RustOwnedWalletDatabase(this._ptr);

  Pointer<Void> clonePointer() {
    return rustCall((status) => uniffi_cdk_ffi_fn_clone_walletdatabase(_ptr, status));
  }

  void dispose() {
    rustCall((status) => uniffi_cdk_ffi_fn_free_walletdatabase(_ptr, status));
  }

  @override
  dynamic noSuchMethod(Invocation invocation) =>
    throw UnimplementedError(
      'Cannot call WalletDatabase methods on a Rust-owned object from Dart');
}

class FfiConverterCallbackInterfaceWalletDatabase {
  static final _handleMap = UniffiHandleMap<WalletDatabase>();
  static bool _vtableInitialized = false;
  static WalletDatabase lift(Pointer<Void> handle) {
    try {
      return _handleMap.get(handle.address);
    } catch (_) {
      // Rust-created object — wrap the pointer in a proxy
      return _RustOwnedWalletDatabase(handle);
    }
  }

  static Pointer<Void> lower(WalletDatabase value) {
    if (value is _RustOwnedWalletDatabase) {
      return value.clonePointer();
    }
    _ensureVTableInitialized();
    final handle = _handleMap.insert(value);
    return Pointer<Void>.fromAddress(handle);
  }

  static void _ensureVTableInitialized() {
    if (!_vtableInitialized) {
      initWalletDatabaseVTable();
      _vtableInitialized = true;
    }
  }

  static LiftRetVal<WalletDatabase> read(Uint8List buf) {
    final handle = buf.buffer.asByteData(buf.offsetInBytes).getInt64(0);
    final pointer = Pointer<Void>.fromAddress(handle);
    return LiftRetVal(lift(pointer), 8);
  }

  static int write(WalletDatabase value, Uint8List buf) {
    final handle = lower(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt64(0, handle.address);
    return 8;
  }

  static int allocationSize(WalletDatabase value) {
    return 8;
  }
}

typedef UniffiForeignFutureCompleteRustBuffer =
    Void Function(Uint64, UniffiForeignFutureResultRustBuffer);
typedef UniffiForeignFutureCompleteRustBufferDart =
    void Function(int, UniffiForeignFutureResultRustBuffer);
typedef UniffiForeignFutureCompleteU64 =
    Void Function(Uint64, UniffiForeignFutureResultU64);
typedef UniffiForeignFutureCompleteU64Dart =
    void Function(int, UniffiForeignFutureResultU64);
typedef UniffiForeignFutureCompleteVoid =
    Void Function(Uint64, UniffiForeignFutureResultVoid);
typedef UniffiForeignFutureCompleteVoidDart =
    void Function(int, UniffiForeignFutureResultVoid);
typedef UniffiForeignFutureCompleteU32 =
    Void Function(Uint64, UniffiForeignFutureResultU32);
typedef UniffiForeignFutureCompleteU32Dart =
    void Function(int, UniffiForeignFutureResultU32);
typedef UniffiForeignFutureCompleteI8 =
    Void Function(Uint64, UniffiForeignFutureResultI8);
typedef UniffiForeignFutureCompleteI8Dart =
    void Function(int, UniffiForeignFutureResultI8);

final class UniffiForeignFutureResultRustBuffer extends Struct {
  external RustBuffer returnValue;
  external RustCallStatus callStatus;
}

final class UniffiForeignFutureResultU64 extends Struct {
  @Uint64()
  external int returnValue;
  external RustCallStatus callStatus;
}

final class UniffiForeignFutureResultVoid extends Struct {
  external RustCallStatus callStatus;
}

final class UniffiForeignFutureResultU32 extends Struct {
  @Uint32()
  external int returnValue;
  external RustCallStatus callStatus;
}

final class UniffiForeignFutureResultI8 extends Struct {
  @Int8()
  external int returnValue;
  external RustCallStatus callStatus;
}

typedef UniffiCallbackInterfaceWalletDatabaseMethod0 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod0Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod1 =
    Void Function(
      Uint64,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod1Dart =
    void Function(
      int,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod2 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod2Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod3 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod3Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod4 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod4Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod5 =
    Void Function(
      Uint64,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod5Dart =
    void Function(
      int,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod6 =
    Void Function(
      Uint64,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod6Dart =
    void Function(
      int,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod7 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod7Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod8 =
    Void Function(
      Uint64,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod8Dart =
    void Function(
      int,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod9 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod9Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod10 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod10Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod11 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod11Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod12 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteU64>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod12Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteU64>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod13 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod13Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod14 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod14Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod15 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod15Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod16 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod16Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod17 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod17Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod18 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod18Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod19 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod19Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod20 =
    Void Function(
      Uint64,
      RustBuffer,
      Int32,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod20Dart =
    void Function(
      int,
      RustBuffer,
      int,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod21 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod21Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod22 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod22Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod23 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod23Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod24 =
    Void Function(
      Uint64,
      RustBuffer,
      Uint32,
      Pointer<NativeFunction<UniffiForeignFutureCompleteU32>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod24Dart =
    void Function(
      int,
      RustBuffer,
      int,
      Pointer<NativeFunction<UniffiForeignFutureCompleteU32>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod25 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod25Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod26 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod26Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod27 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod27Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod28 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod28Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod29 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod29Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod30 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod30Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod31 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod31Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod32 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod32Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod33 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod33Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod34 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod34Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod35 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod35Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod36 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteI8>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod36Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteI8>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod37 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod37Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod38 =
    Void Function(
      Uint64,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod38Dart =
    void Function(
      int,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod39 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod39Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod40 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod40Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod41 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod41Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod42 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod42Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod43 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod43Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod44 =
    Void Function(
      Uint64,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod44Dart =
    void Function(
      int,
      RustBuffer,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod45 =
    Void Function(
      Uint64,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      Uint64,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseMethod45Dart =
    void Function(
      int,
      RustBuffer,
      Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>>,
      int,
      Pointer<UniffiForeignFuture>,
    );
typedef UniffiCallbackInterfaceWalletDatabaseFree = Void Function(Uint64);
typedef UniffiCallbackInterfaceWalletDatabaseFreeDart = void Function(int);
typedef UniffiCallbackInterfaceWalletDatabaseClone = Uint64 Function(Uint64);
typedef UniffiCallbackInterfaceWalletDatabaseCloneDart = int Function(int);

final class UniffiVTableCallbackInterfaceWalletDatabase extends Struct {
  external Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseFree>>
  uniffiFree;
  external Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseClone>>
  uniffiClone;
  external Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod0>>
  getMint;
  external Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod1>>
  getMints;
  external Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod2>>
  getMintKeysets;
  external Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod3>>
  getKeysetById;
  external Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod4>>
  getMintQuote;
  external Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod5>>
  getMintQuotes;
  external Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod6>>
  getUnissuedMintQuotes;
  external Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod7>>
  getMeltQuote;
  external Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod8>>
  getMeltQuotes;
  external Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod9>>
  getKeys;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod10>
  >
  getProofs;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod11>
  >
  getProofsByYs;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod12>
  >
  getBalance;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod13>
  >
  getTransaction;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod14>
  >
  listTransactions;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod15>
  >
  kvRead;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod16>
  >
  kvList;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod17>
  >
  kvWrite;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod18>
  >
  kvRemove;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod19>
  >
  updateProofs;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod20>
  >
  updateProofsState;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod21>
  >
  addTransaction;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod22>
  >
  removeTransaction;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod23>
  >
  updateMintUrl;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod24>
  >
  incrementKeysetCounter;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod25>
  >
  addMint;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod26>
  >
  removeMint;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod27>
  >
  addMintKeysets;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod28>
  >
  addMintQuote;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod29>
  >
  removeMintQuote;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod30>
  >
  addMeltQuote;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod31>
  >
  removeMeltQuote;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod32>
  >
  addKeys;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod33>
  >
  removeKeys;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod34>
  >
  addSaga;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod35>
  >
  getSaga;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod36>
  >
  updateSaga;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod37>
  >
  deleteSaga;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod38>
  >
  getIncompleteSagas;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod39>
  >
  reserveProofs;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod40>
  >
  releaseProofs;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod41>
  >
  getReservedProofs;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod42>
  >
  reserveMeltQuote;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod43>
  >
  releaseMeltQuote;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod44>
  >
  reserveMintQuote;
  external Pointer<
    NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod45>
  >
  releaseMintQuote;
}

void walletDatabaseGetMint(
  int uniffiHandle,
  RustBuffer mintUrl,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterMintUrl.lift(mintUrl);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getMint(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterOptionalMintInfo.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod0>>
walletDatabaseGetMintPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod0>(
      walletDatabaseGetMint,
    );
void walletDatabaseGetMints(
  int uniffiHandle,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getMints();
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue =
            FfiConverterMapMintUrlToOptionalMintInfo.lower(result);
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod1>>
walletDatabaseGetMintsPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod1>(
      walletDatabaseGetMints,
    );
void walletDatabaseGetMintKeysets(
  int uniffiHandle,
  RustBuffer mintUrl,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterMintUrl.lift(mintUrl);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getMintKeysets(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue =
            FfiConverterOptionalSequenceKeySetInfo.lower(result);
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod2>>
walletDatabaseGetMintKeysetsPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod2>(
      walletDatabaseGetMintKeysets,
    );
void walletDatabaseGetKeysetById(
  int uniffiHandle,
  RustBuffer keysetId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterId.lift(keysetId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getKeysetById(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterOptionalKeySetInfo.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod3>>
walletDatabaseGetKeysetByIdPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod3>(
      walletDatabaseGetKeysetById,
    );
void walletDatabaseGetMintQuote(
  int uniffiHandle,
  RustBuffer quoteId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(quoteId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getMintQuote(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterOptionalMintQuote.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod4>>
walletDatabaseGetMintQuotePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod4>(
      walletDatabaseGetMintQuote,
    );
void walletDatabaseGetMintQuotes(
  int uniffiHandle,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getMintQuotes();
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterSequenceMintQuote.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod5>>
walletDatabaseGetMintQuotesPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod5>(
      walletDatabaseGetMintQuotes,
    );
void walletDatabaseGetUnissuedMintQuotes(
  int uniffiHandle,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getUnissuedMintQuotes();
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterSequenceMintQuote.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod6>>
walletDatabaseGetUnissuedMintQuotesPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod6>(
      walletDatabaseGetUnissuedMintQuotes,
    );
void walletDatabaseGetMeltQuote(
  int uniffiHandle,
  RustBuffer quoteId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(quoteId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getMeltQuote(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterOptionalMeltQuote.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod7>>
walletDatabaseGetMeltQuotePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod7>(
      walletDatabaseGetMeltQuote,
    );
void walletDatabaseGetMeltQuotes(
  int uniffiHandle,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getMeltQuotes();
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterSequenceMeltQuote.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod8>>
walletDatabaseGetMeltQuotesPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod8>(
      walletDatabaseGetMeltQuotes,
    );
void walletDatabaseGetKeys(
  int uniffiHandle,
  RustBuffer id,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterId.lift(id);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getKeys(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterOptionalKeys.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod9>>
walletDatabaseGetKeysPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod9>(
      walletDatabaseGetKeys,
    );
void walletDatabaseGetProofs(
  int uniffiHandle,
  RustBuffer mintUrl,
  RustBuffer unit,
  RustBuffer state,
  RustBuffer spendingConditions,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterOptionalMintUrl.lift(mintUrl);
  final arg1 = FfiConverterOptionalCurrencyUnit.lift(unit);
  final arg2 = FfiConverterOptionalSequenceProofState.lift(state);
  final arg3 = FfiConverterOptionalSequenceSpendingConditions.lift(
    spendingConditions,
  );
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getProofs(arg0, arg1, arg2, arg3);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterSequenceProofInfo.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod10>>
walletDatabaseGetProofsPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod10>(
      walletDatabaseGetProofs,
    );
void walletDatabaseGetProofsByYs(
  int uniffiHandle,
  RustBuffer ys,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterSequencePublicKey.lift(ys);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getProofsByYs(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterSequenceProofInfo.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod11>>
walletDatabaseGetProofsByYsPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod11>(
      walletDatabaseGetProofsByYs,
    );
void walletDatabaseGetBalance(
  int uniffiHandle,
  RustBuffer mintUrl,
  RustBuffer unit,
  RustBuffer state,
  Pointer<NativeFunction<UniffiForeignFutureCompleteU64>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterOptionalMintUrl.lift(mintUrl);
  final arg1 = FfiConverterOptionalCurrencyUnit.lift(unit);
  final arg2 = FfiConverterOptionalSequenceProofState.lift(state);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteU64Dart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getBalance(arg0, arg1, arg2);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultU64>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterUInt64.lower(result);
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultU64>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod12>>
walletDatabaseGetBalancePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod12>(
      walletDatabaseGetBalance,
    );
void walletDatabaseGetTransaction(
  int uniffiHandle,
  RustBuffer transactionId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterTransactionId.lift(transactionId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getTransaction(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterOptionalTransaction.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod13>>
walletDatabaseGetTransactionPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod13>(
      walletDatabaseGetTransaction,
    );
void walletDatabaseListTransactions(
  int uniffiHandle,
  RustBuffer mintUrl,
  RustBuffer direction,
  RustBuffer unit,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterOptionalMintUrl.lift(mintUrl);
  final arg1 = FfiConverterOptionalTransactionDirection.lift(direction);
  final arg2 = FfiConverterOptionalCurrencyUnit.lift(unit);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.listTransactions(arg0, arg1, arg2);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterSequenceTransaction.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod14>>
walletDatabaseListTransactionsPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod14>(
      walletDatabaseListTransactions,
    );
void walletDatabaseKvRead(
  int uniffiHandle,
  RustBuffer primaryNamespace,
  RustBuffer secondaryNamespace,
  RustBuffer key,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(primaryNamespace);
  final arg1 = FfiConverterString.lift(secondaryNamespace);
  final arg2 = FfiConverterString.lift(key);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.kvRead(arg0, arg1, arg2);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterOptionalUint8List.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod15>>
walletDatabaseKvReadPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod15>(
      walletDatabaseKvRead,
    );
void walletDatabaseKvList(
  int uniffiHandle,
  RustBuffer primaryNamespace,
  RustBuffer secondaryNamespace,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(primaryNamespace);
  final arg1 = FfiConverterString.lift(secondaryNamespace);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.kvList(arg0, arg1);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterSequenceString.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod16>>
walletDatabaseKvListPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod16>(
      walletDatabaseKvList,
    );
void walletDatabaseKvWrite(
  int uniffiHandle,
  RustBuffer primaryNamespace,
  RustBuffer secondaryNamespace,
  RustBuffer key,
  RustBuffer value,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(primaryNamespace);
  final arg1 = FfiConverterString.lift(secondaryNamespace);
  final arg2 = FfiConverterString.lift(key);
  final arg3 = FfiConverterUint8List.lift(value);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.kvWrite(arg0, arg1, arg2, arg3);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod17>>
walletDatabaseKvWritePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod17>(
      walletDatabaseKvWrite,
    );
void walletDatabaseKvRemove(
  int uniffiHandle,
  RustBuffer primaryNamespace,
  RustBuffer secondaryNamespace,
  RustBuffer key,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(primaryNamespace);
  final arg1 = FfiConverterString.lift(secondaryNamespace);
  final arg2 = FfiConverterString.lift(key);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.kvRemove(arg0, arg1, arg2);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod18>>
walletDatabaseKvRemovePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod18>(
      walletDatabaseKvRemove,
    );
void walletDatabaseUpdateProofs(
  int uniffiHandle,
  RustBuffer added,
  RustBuffer removedYs,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterSequenceProofInfo.lift(added);
  final arg1 = FfiConverterSequencePublicKey.lift(removedYs);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.updateProofs(arg0, arg1);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod19>>
walletDatabaseUpdateProofsPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod19>(
      walletDatabaseUpdateProofs,
    );
void walletDatabaseUpdateProofsState(
  int uniffiHandle,
  RustBuffer ys,
  int state,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterSequencePublicKey.lift(ys);
  final arg1 = FfiConverterProofState.read(createUint8ListFromInt(state)).value;
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.updateProofsState(arg0, arg1);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod20>>
walletDatabaseUpdateProofsStatePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod20>(
      walletDatabaseUpdateProofsState,
    );
void walletDatabaseAddTransaction(
  int uniffiHandle,
  RustBuffer transaction,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterTransaction.lift(transaction);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.addTransaction(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod21>>
walletDatabaseAddTransactionPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod21>(
      walletDatabaseAddTransaction,
    );
void walletDatabaseRemoveTransaction(
  int uniffiHandle,
  RustBuffer transactionId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterTransactionId.lift(transactionId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.removeTransaction(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod22>>
walletDatabaseRemoveTransactionPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod22>(
      walletDatabaseRemoveTransaction,
    );
void walletDatabaseUpdateMintUrl(
  int uniffiHandle,
  RustBuffer oldMintUrl,
  RustBuffer newMintUrl,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterMintUrl.lift(oldMintUrl);
  final arg1 = FfiConverterMintUrl.lift(newMintUrl);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.updateMintUrl(arg0, arg1);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod23>>
walletDatabaseUpdateMintUrlPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod23>(
      walletDatabaseUpdateMintUrl,
    );
void walletDatabaseIncrementKeysetCounter(
  int uniffiHandle,
  RustBuffer keysetId,
  int count,
  Pointer<NativeFunction<UniffiForeignFutureCompleteU32>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterId.lift(keysetId);
  final arg1 = FfiConverterUInt32.lift(count);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteU32Dart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.incrementKeysetCounter(arg0, arg1);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultU32>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterUInt32.lower(result);
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultU32>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod24>>
walletDatabaseIncrementKeysetCounterPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod24>(
      walletDatabaseIncrementKeysetCounter,
    );
void walletDatabaseAddMint(
  int uniffiHandle,
  RustBuffer mintUrl,
  RustBuffer mintInfo,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterMintUrl.lift(mintUrl);
  final arg1 = FfiConverterOptionalMintInfo.lift(mintInfo);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.addMint(arg0, arg1);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod25>>
walletDatabaseAddMintPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod25>(
      walletDatabaseAddMint,
    );
void walletDatabaseRemoveMint(
  int uniffiHandle,
  RustBuffer mintUrl,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterMintUrl.lift(mintUrl);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.removeMint(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod26>>
walletDatabaseRemoveMintPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod26>(
      walletDatabaseRemoveMint,
    );
void walletDatabaseAddMintKeysets(
  int uniffiHandle,
  RustBuffer mintUrl,
  RustBuffer keysets,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterMintUrl.lift(mintUrl);
  final arg1 = FfiConverterSequenceKeySetInfo.lift(keysets);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.addMintKeysets(arg0, arg1);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod27>>
walletDatabaseAddMintKeysetsPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod27>(
      walletDatabaseAddMintKeysets,
    );
void walletDatabaseAddMintQuote(
  int uniffiHandle,
  RustBuffer quote,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterMintQuote.lift(quote);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.addMintQuote(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod28>>
walletDatabaseAddMintQuotePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod28>(
      walletDatabaseAddMintQuote,
    );
void walletDatabaseRemoveMintQuote(
  int uniffiHandle,
  RustBuffer quoteId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(quoteId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.removeMintQuote(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod29>>
walletDatabaseRemoveMintQuotePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod29>(
      walletDatabaseRemoveMintQuote,
    );
void walletDatabaseAddMeltQuote(
  int uniffiHandle,
  RustBuffer quote,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterMeltQuote.lift(quote);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.addMeltQuote(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod30>>
walletDatabaseAddMeltQuotePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod30>(
      walletDatabaseAddMeltQuote,
    );
void walletDatabaseRemoveMeltQuote(
  int uniffiHandle,
  RustBuffer quoteId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(quoteId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.removeMeltQuote(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod31>>
walletDatabaseRemoveMeltQuotePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod31>(
      walletDatabaseRemoveMeltQuote,
    );
void walletDatabaseAddKeys(
  int uniffiHandle,
  RustBuffer keyset,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterKeySet.lift(keyset);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.addKeys(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod32>>
walletDatabaseAddKeysPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod32>(
      walletDatabaseAddKeys,
    );
void walletDatabaseRemoveKeys(
  int uniffiHandle,
  RustBuffer id,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterId.lift(id);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.removeKeys(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod33>>
walletDatabaseRemoveKeysPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod33>(
      walletDatabaseRemoveKeys,
    );
void walletDatabaseAddSaga(
  int uniffiHandle,
  RustBuffer sagaJson,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(sagaJson);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.addSaga(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod34>>
walletDatabaseAddSagaPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod34>(
      walletDatabaseAddSaga,
    );
void walletDatabaseGetSaga(
  int uniffiHandle,
  RustBuffer id,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(id);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getSaga(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterOptionalString.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod35>>
walletDatabaseGetSagaPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod35>(
      walletDatabaseGetSaga,
    );
void walletDatabaseUpdateSaga(
  int uniffiHandle,
  RustBuffer sagaJson,
  Pointer<NativeFunction<UniffiForeignFutureCompleteI8>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(sagaJson);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteI8Dart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.updateSaga(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultI8>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterBool.lower(result);
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultI8>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod36>>
walletDatabaseUpdateSagaPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod36>(
      walletDatabaseUpdateSaga,
    );
void walletDatabaseDeleteSaga(
  int uniffiHandle,
  RustBuffer id,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(id);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.deleteSaga(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod37>>
walletDatabaseDeleteSagaPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod37>(
      walletDatabaseDeleteSaga,
    );
void walletDatabaseGetIncompleteSagas(
  int uniffiHandle,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getIncompleteSagas();
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterSequenceString.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod38>>
walletDatabaseGetIncompleteSagasPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod38>(
      walletDatabaseGetIncompleteSagas,
    );
void walletDatabaseReserveProofs(
  int uniffiHandle,
  RustBuffer ys,
  RustBuffer operationId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterSequencePublicKey.lift(ys);
  final arg1 = FfiConverterString.lift(operationId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.reserveProofs(arg0, arg1);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod39>>
walletDatabaseReserveProofsPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod39>(
      walletDatabaseReserveProofs,
    );
void walletDatabaseReleaseProofs(
  int uniffiHandle,
  RustBuffer operationId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(operationId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.releaseProofs(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod40>>
walletDatabaseReleaseProofsPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod40>(
      walletDatabaseReleaseProofs,
    );
void walletDatabaseGetReservedProofs(
  int uniffiHandle,
  RustBuffer operationId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteRustBuffer>>
  uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(operationId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteRustBufferDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.getReservedProofs(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.returnValue = FfiConverterSequenceProofInfo.lower(
          result,
        );
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultRustBuffer>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod41>>
walletDatabaseGetReservedProofsPointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod41>(
      walletDatabaseGetReservedProofs,
    );
void walletDatabaseReserveMeltQuote(
  int uniffiHandle,
  RustBuffer quoteId,
  RustBuffer operationId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(quoteId);
  final arg1 = FfiConverterString.lift(operationId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.reserveMeltQuote(arg0, arg1);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod42>>
walletDatabaseReserveMeltQuotePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod42>(
      walletDatabaseReserveMeltQuote,
    );
void walletDatabaseReleaseMeltQuote(
  int uniffiHandle,
  RustBuffer operationId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(operationId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.releaseMeltQuote(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod43>>
walletDatabaseReleaseMeltQuotePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod43>(
      walletDatabaseReleaseMeltQuote,
    );
void walletDatabaseReserveMintQuote(
  int uniffiHandle,
  RustBuffer quoteId,
  RustBuffer operationId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(quoteId);
  final arg1 = FfiConverterString.lift(operationId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.reserveMintQuote(arg0, arg1);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod44>>
walletDatabaseReserveMintQuotePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod44>(
      walletDatabaseReserveMintQuote,
    );
void walletDatabaseReleaseMintQuote(
  int uniffiHandle,
  RustBuffer operationId,
  Pointer<NativeFunction<UniffiForeignFutureCompleteVoid>> uniffiFutureCallback,
  int uniffiCallbackData,
  Pointer<UniffiForeignFuture> outReturn,
) {
  final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
    uniffiHandle,
  );
  final arg0 = FfiConverterString.lift(operationId);
  final callback = uniffiFutureCallback
      .asFunction<UniffiForeignFutureCompleteVoidDart>();
  final _futureState = _UniffiForeignFutureState();
  final handle = _uniffiForeignFutureHandleMap.insert(_futureState);
  outReturn.ref.handle = handle;
  outReturn.ref.free = _uniffiForeignFutureFreePointer;
  () async {
    try {
      final result = await obj.releaseMintQuote(arg0);
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_SUCCESS;
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    } catch (e) {
      final removedState = _uniffiForeignFutureHandleMap.maybeRemove(handle);
      final effectiveState = removedState ?? _futureState;
      if (effectiveState.cancelled) {
        return;
      }
      effectiveState.cancelled = true;
      final resultStructPtr = calloc<UniffiForeignFutureResultVoid>();
      try {
        resultStructPtr.ref.callStatus.code = CALL_UNEXPECTED_ERROR;
        resultStructPtr.ref.callStatus.errorBuf = FfiConverterString.lower(
          e.toString(),
        );
        callback(uniffiCallbackData, resultStructPtr.ref);
      } finally {
        calloc.free(resultStructPtr);
      }
    }
  }();
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseMethod45>>
walletDatabaseReleaseMintQuotePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseMethod45>(
      walletDatabaseReleaseMintQuote,
    );
void walletDatabaseFreeCallback(int handle) {
  try {
    FfiConverterCallbackInterfaceWalletDatabase._handleMap.remove(handle);
  } catch (e) {}
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseFree>>
walletDatabaseFreePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseFree>(
      walletDatabaseFreeCallback,
    );
int walletDatabaseCloneCallback(int handle) {
  try {
    final obj = FfiConverterCallbackInterfaceWalletDatabase._handleMap.get(
      handle,
    );
    final newHandle = FfiConverterCallbackInterfaceWalletDatabase._handleMap
        .insert(obj);
    return newHandle;
  } catch (e) {
    return 0;
  }
}

final Pointer<NativeFunction<UniffiCallbackInterfaceWalletDatabaseClone>>
walletDatabaseClonePointer =
    Pointer.fromFunction<UniffiCallbackInterfaceWalletDatabaseClone>(
      walletDatabaseCloneCallback,
      0,
    );
late final Pointer<UniffiVTableCallbackInterfaceWalletDatabase>
walletDatabaseVTable;
void initWalletDatabaseVTable() {
  if (FfiConverterCallbackInterfaceWalletDatabase._vtableInitialized) {
    return;
  }
  walletDatabaseVTable = calloc<UniffiVTableCallbackInterfaceWalletDatabase>();
  walletDatabaseVTable.ref.uniffiFree = walletDatabaseFreePointer;
  walletDatabaseVTable.ref.uniffiClone = walletDatabaseClonePointer;
  walletDatabaseVTable.ref.getMint = walletDatabaseGetMintPointer;
  walletDatabaseVTable.ref.getMints = walletDatabaseGetMintsPointer;
  walletDatabaseVTable.ref.getMintKeysets = walletDatabaseGetMintKeysetsPointer;
  walletDatabaseVTable.ref.getKeysetById = walletDatabaseGetKeysetByIdPointer;
  walletDatabaseVTable.ref.getMintQuote = walletDatabaseGetMintQuotePointer;
  walletDatabaseVTable.ref.getMintQuotes = walletDatabaseGetMintQuotesPointer;
  walletDatabaseVTable.ref.getUnissuedMintQuotes =
      walletDatabaseGetUnissuedMintQuotesPointer;
  walletDatabaseVTable.ref.getMeltQuote = walletDatabaseGetMeltQuotePointer;
  walletDatabaseVTable.ref.getMeltQuotes = walletDatabaseGetMeltQuotesPointer;
  walletDatabaseVTable.ref.getKeys = walletDatabaseGetKeysPointer;
  walletDatabaseVTable.ref.getProofs = walletDatabaseGetProofsPointer;
  walletDatabaseVTable.ref.getProofsByYs = walletDatabaseGetProofsByYsPointer;
  walletDatabaseVTable.ref.getBalance = walletDatabaseGetBalancePointer;
  walletDatabaseVTable.ref.getTransaction = walletDatabaseGetTransactionPointer;
  walletDatabaseVTable.ref.listTransactions =
      walletDatabaseListTransactionsPointer;
  walletDatabaseVTable.ref.kvRead = walletDatabaseKvReadPointer;
  walletDatabaseVTable.ref.kvList = walletDatabaseKvListPointer;
  walletDatabaseVTable.ref.kvWrite = walletDatabaseKvWritePointer;
  walletDatabaseVTable.ref.kvRemove = walletDatabaseKvRemovePointer;
  walletDatabaseVTable.ref.updateProofs = walletDatabaseUpdateProofsPointer;
  walletDatabaseVTable.ref.updateProofsState =
      walletDatabaseUpdateProofsStatePointer;
  walletDatabaseVTable.ref.addTransaction = walletDatabaseAddTransactionPointer;
  walletDatabaseVTable.ref.removeTransaction =
      walletDatabaseRemoveTransactionPointer;
  walletDatabaseVTable.ref.updateMintUrl = walletDatabaseUpdateMintUrlPointer;
  walletDatabaseVTable.ref.incrementKeysetCounter =
      walletDatabaseIncrementKeysetCounterPointer;
  walletDatabaseVTable.ref.addMint = walletDatabaseAddMintPointer;
  walletDatabaseVTable.ref.removeMint = walletDatabaseRemoveMintPointer;
  walletDatabaseVTable.ref.addMintKeysets = walletDatabaseAddMintKeysetsPointer;
  walletDatabaseVTable.ref.addMintQuote = walletDatabaseAddMintQuotePointer;
  walletDatabaseVTable.ref.removeMintQuote =
      walletDatabaseRemoveMintQuotePointer;
  walletDatabaseVTable.ref.addMeltQuote = walletDatabaseAddMeltQuotePointer;
  walletDatabaseVTable.ref.removeMeltQuote =
      walletDatabaseRemoveMeltQuotePointer;
  walletDatabaseVTable.ref.addKeys = walletDatabaseAddKeysPointer;
  walletDatabaseVTable.ref.removeKeys = walletDatabaseRemoveKeysPointer;
  walletDatabaseVTable.ref.addSaga = walletDatabaseAddSagaPointer;
  walletDatabaseVTable.ref.getSaga = walletDatabaseGetSagaPointer;
  walletDatabaseVTable.ref.updateSaga = walletDatabaseUpdateSagaPointer;
  walletDatabaseVTable.ref.deleteSaga = walletDatabaseDeleteSagaPointer;
  walletDatabaseVTable.ref.getIncompleteSagas =
      walletDatabaseGetIncompleteSagasPointer;
  walletDatabaseVTable.ref.reserveProofs = walletDatabaseReserveProofsPointer;
  walletDatabaseVTable.ref.releaseProofs = walletDatabaseReleaseProofsPointer;
  walletDatabaseVTable.ref.getReservedProofs =
      walletDatabaseGetReservedProofsPointer;
  walletDatabaseVTable.ref.reserveMeltQuote =
      walletDatabaseReserveMeltQuotePointer;
  walletDatabaseVTable.ref.releaseMeltQuote =
      walletDatabaseReleaseMeltQuotePointer;
  walletDatabaseVTable.ref.reserveMintQuote =
      walletDatabaseReserveMintQuotePointer;
  walletDatabaseVTable.ref.releaseMintQuote =
      walletDatabaseReleaseMintQuotePointer;
  rustCall((status) {
    uniffi_cdk_ffi_fn_init_callback_vtable_walletdatabase(walletDatabaseVTable);
    checkCallStatus(NullRustCallStatusErrorHandler(), status);
  });
  FfiConverterCallbackInterfaceWalletDatabase._vtableInitialized = true;
}

abstract class WalletPostgresDatabaseInterface {
  Future<void> addKeys({required KeySet keyset});
  Future<void> addMeltQuote({required MeltQuote quote});
  Future<void> addMint({required MintUrl mintUrl, required MintInfo? mintInfo});
  Future<void> addMintKeysets({
    required MintUrl mintUrl,
    required List<KeySetInfo> keysets,
  });
  Future<void> addMintQuote({required MintQuote quote});
  Future<void> addSaga({required String sagaJson});
  Future<void> addTransaction({required Transaction transaction});
  Future<void> deleteSaga({required String id});
  Future<int> getBalance({
    required MintUrl? mintUrl,
    required CurrencyUnit? unit,
    required List<ProofState>? state,
  });
  Future<List<String>> getIncompleteSagas();
  Future<Keys?> getKeys({required Id id});
  Future<KeySetInfo?> getKeysetById({required Id keysetId});
  Future<MeltQuote?> getMeltQuote({required String quoteId});
  Future<List<MeltQuote>> getMeltQuotes();
  Future<MintInfo?> getMint({required MintUrl mintUrl});
  Future<List<KeySetInfo>?> getMintKeysets({required MintUrl mintUrl});
  Future<MintQuote?> getMintQuote({required String quoteId});
  Future<List<MintQuote>> getMintQuotes();
  Future<Map<MintUrl, MintInfo?>> getMints();
  Future<List<ProofInfo>> getProofs({
    required MintUrl? mintUrl,
    required CurrencyUnit? unit,
    required List<ProofState>? state,
    required List<SpendingConditions>? spendingConditions,
  });
  Future<List<ProofInfo>> getProofsByYs({required List<PublicKey> ys});
  Future<List<ProofInfo>> getReservedProofs({required String operationId});
  Future<String?> getSaga({required String id});
  Future<Transaction?> getTransaction({required TransactionId transactionId});
  Future<List<MintQuote>> getUnissuedMintQuotes();
  Future<int> incrementKeysetCounter({
    required Id keysetId,
    required int count,
  });
  Future<List<String>> kvList({
    required String primaryNamespace,
    required String secondaryNamespace,
  });
  Future<Uint8List?> kvRead({
    required String primaryNamespace,
    required String secondaryNamespace,
    required String key,
  });
  Future<void> kvRemove({
    required String primaryNamespace,
    required String secondaryNamespace,
    required String key,
  });
  Future<void> kvWrite({
    required String primaryNamespace,
    required String secondaryNamespace,
    required String key,
    required Uint8List value,
  });
  Future<List<Transaction>> listTransactions({
    required MintUrl? mintUrl,
    required TransactionDirection? direction,
    required CurrencyUnit? unit,
  });
  Future<void> releaseMeltQuote({required String operationId});
  Future<void> releaseMintQuote({required String operationId});
  Future<void> releaseProofs({required String operationId});
  Future<void> removeKeys({required Id id});
  Future<void> removeMeltQuote({required String quoteId});
  Future<void> removeMint({required MintUrl mintUrl});
  Future<void> removeMintQuote({required String quoteId});
  Future<void> removeTransaction({required TransactionId transactionId});
  Future<void> reserveMeltQuote({
    required String quoteId,
    required String operationId,
  });
  Future<void> reserveMintQuote({
    required String quoteId,
    required String operationId,
  });
  Future<void> reserveProofs({
    required List<PublicKey> ys,
    required String operationId,
  });
  Future<void> updateMintUrl({
    required MintUrl oldMintUrl,
    required MintUrl newMintUrl,
  });
  Future<void> updateProofs({
    required List<ProofInfo> added,
    required List<PublicKey> removedYs,
  });
  Future<void> updateProofsState({
    required List<PublicKey> ys,
    required ProofState state,
  });
  Future<bool> updateSaga({required String sagaJson});
}

final _WalletPostgresDatabaseFinalizer = Finalizer<Pointer<Void>>((ptr) {
  rustCall(
    (status) => uniffi_cdk_ffi_fn_free_walletpostgresdatabase(ptr, status),
  );
});

class WalletPostgresDatabase implements WalletPostgresDatabaseInterface {
  late final Pointer<Void> _ptr;
  WalletPostgresDatabase._(this._ptr) {
    _WalletPostgresDatabaseFinalizer.attach(this, _ptr, detach: this);
  }
  WalletPostgresDatabase({required String url})
    : _ptr = rustCall(
        (status) => uniffi_cdk_ffi_fn_constructor_walletpostgresdatabase_new(
          FfiConverterString.lower(url),
          status,
        ),
        ffiExceptionErrorHandler,
      ) {
    _WalletPostgresDatabaseFinalizer.attach(this, _ptr, detach: this);
  }
  factory WalletPostgresDatabase.lift(Pointer<Void> ptr) {
    return WalletPostgresDatabase._(ptr);
  }
  static Pointer<Void> lower(WalletPostgresDatabase value) {
    return value.uniffiClonePointer();
  }

  Pointer<Void> uniffiClonePointer() {
    return rustCall(
      (status) => uniffi_cdk_ffi_fn_clone_walletpostgresdatabase(_ptr, status),
    );
  }

  static int allocationSize(WalletPostgresDatabase value) {
    return 8;
  }

  static LiftRetVal<WalletPostgresDatabase> read(Uint8List buf) {
    final handle = buf.buffer.asByteData(buf.offsetInBytes).getInt64(0);
    final pointer = Pointer<Void>.fromAddress(handle);
    return LiftRetVal(WalletPostgresDatabase.lift(pointer), 8);
  }

  static int write(WalletPostgresDatabase value, Uint8List buf) {
    final handle = lower(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt64(0, handle.address);
    return 8;
  }

  void dispose() {
    _WalletPostgresDatabaseFinalizer.detach(this);
    rustCall(
      (status) => uniffi_cdk_ffi_fn_free_walletpostgresdatabase(_ptr, status),
    );
  }

  Future<void> addKeys({required KeySet keyset}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_keys(
        uniffiClonePointer(),
        FfiConverterKeySet.lower(keyset),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> addMeltQuote({required MeltQuote quote}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_melt_quote(
        uniffiClonePointer(),
        FfiConverterMeltQuote.lower(quote),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> addMint({
    required MintUrl mintUrl,
    required MintInfo? mintInfo,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_mint(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
        FfiConverterOptionalMintInfo.lower(mintInfo),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> addMintKeysets({
    required MintUrl mintUrl,
    required List<KeySetInfo> keysets,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_mint_keysets(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
        FfiConverterSequenceKeySetInfo.lower(keysets),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> addMintQuote({required MintQuote quote}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_mint_quote(
        uniffiClonePointer(),
        FfiConverterMintQuote.lower(quote),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> addSaga({required String sagaJson}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_saga(
        uniffiClonePointer(),
        FfiConverterString.lower(sagaJson),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> addTransaction({required Transaction transaction}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_transaction(
        uniffiClonePointer(),
        FfiConverterTransaction.lower(transaction),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> deleteSaga({required String id}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_delete_saga(
        uniffiClonePointer(),
        FfiConverterString.lower(id),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<int> getBalance({
    required MintUrl? mintUrl,
    required CurrencyUnit? unit,
    required List<ProofState>? state,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_balance(
        uniffiClonePointer(),
        FfiConverterOptionalMintUrl.lower(mintUrl),
        FfiConverterOptionalCurrencyUnit.lower(unit),
        FfiConverterOptionalSequenceProofState.lower(state),
      ),
      ffi_cdk_ffi_rust_future_poll_u64,
      ffi_cdk_ffi_rust_future_complete_u64,
      ffi_cdk_ffi_rust_future_free_u64,
      FfiConverterUInt64.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<String>> getIncompleteSagas() {
    return uniffiRustCallAsync(
      () =>
          uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_incomplete_sagas(
            uniffiClonePointer(),
          ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceString.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Keys?> getKeys({required Id id}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_keys(
        uniffiClonePointer(),
        FfiConverterId.lower(id),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalKeys.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<KeySetInfo?> getKeysetById({required Id keysetId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_keyset_by_id(
        uniffiClonePointer(),
        FfiConverterId.lower(keysetId),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalKeySetInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MeltQuote?> getMeltQuote({required String quoteId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_melt_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalMeltQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<MeltQuote>> getMeltQuotes() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_melt_quotes(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceMeltQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MintInfo?> getMint({required MintUrl mintUrl}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_mint(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalMintInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<KeySetInfo>?> getMintKeysets({required MintUrl mintUrl}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_mint_keysets(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalSequenceKeySetInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MintQuote?> getMintQuote({required String quoteId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_mint_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalMintQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<MintQuote>> getMintQuotes() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_mint_quotes(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceMintQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Map<MintUrl, MintInfo?>> getMints() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_mints(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterMapMintUrlToOptionalMintInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<ProofInfo>> getProofs({
    required MintUrl? mintUrl,
    required CurrencyUnit? unit,
    required List<ProofState>? state,
    required List<SpendingConditions>? spendingConditions,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_proofs(
        uniffiClonePointer(),
        FfiConverterOptionalMintUrl.lower(mintUrl),
        FfiConverterOptionalCurrencyUnit.lower(unit),
        FfiConverterOptionalSequenceProofState.lower(state),
        FfiConverterOptionalSequenceSpendingConditions.lower(
          spendingConditions,
        ),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceProofInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<ProofInfo>> getProofsByYs({required List<PublicKey> ys}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_proofs_by_ys(
        uniffiClonePointer(),
        FfiConverterSequencePublicKey.lower(ys),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceProofInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<ProofInfo>> getReservedProofs({required String operationId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_reserved_proofs(
        uniffiClonePointer(),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceProofInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<String?> getSaga({required String id}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_saga(
        uniffiClonePointer(),
        FfiConverterString.lower(id),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalString.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Transaction?> getTransaction({required TransactionId transactionId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_transaction(
        uniffiClonePointer(),
        FfiConverterTransactionId.lower(transactionId),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalTransaction.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<MintQuote>> getUnissuedMintQuotes() {
    return uniffiRustCallAsync(
      () =>
          uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_unissued_mint_quotes(
            uniffiClonePointer(),
          ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceMintQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<int> incrementKeysetCounter({
    required Id keysetId,
    required int count,
  }) {
    return uniffiRustCallAsync(
      () =>
          uniffi_cdk_ffi_fn_method_walletpostgresdatabase_increment_keyset_counter(
            uniffiClonePointer(),
            FfiConverterId.lower(keysetId),
            FfiConverterUInt32.lower(count),
          ),
      ffi_cdk_ffi_rust_future_poll_u32,
      ffi_cdk_ffi_rust_future_complete_u32,
      ffi_cdk_ffi_rust_future_free_u32,
      FfiConverterUInt32.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<String>> kvList({
    required String primaryNamespace,
    required String secondaryNamespace,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_kv_list(
        uniffiClonePointer(),
        FfiConverterString.lower(primaryNamespace),
        FfiConverterString.lower(secondaryNamespace),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceString.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Uint8List?> kvRead({
    required String primaryNamespace,
    required String secondaryNamespace,
    required String key,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_kv_read(
        uniffiClonePointer(),
        FfiConverterString.lower(primaryNamespace),
        FfiConverterString.lower(secondaryNamespace),
        FfiConverterString.lower(key),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalUint8List.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<void> kvRemove({
    required String primaryNamespace,
    required String secondaryNamespace,
    required String key,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_kv_remove(
        uniffiClonePointer(),
        FfiConverterString.lower(primaryNamespace),
        FfiConverterString.lower(secondaryNamespace),
        FfiConverterString.lower(key),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> kvWrite({
    required String primaryNamespace,
    required String secondaryNamespace,
    required String key,
    required Uint8List value,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_kv_write(
        uniffiClonePointer(),
        FfiConverterString.lower(primaryNamespace),
        FfiConverterString.lower(secondaryNamespace),
        FfiConverterString.lower(key),
        FfiConverterUint8List.lower(value),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<List<Transaction>> listTransactions({
    required MintUrl? mintUrl,
    required TransactionDirection? direction,
    required CurrencyUnit? unit,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_list_transactions(
        uniffiClonePointer(),
        FfiConverterOptionalMintUrl.lower(mintUrl),
        FfiConverterOptionalTransactionDirection.lower(direction),
        FfiConverterOptionalCurrencyUnit.lower(unit),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceTransaction.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<void> releaseMeltQuote({required String operationId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_release_melt_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> releaseMintQuote({required String operationId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_release_mint_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> releaseProofs({required String operationId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_release_proofs(
        uniffiClonePointer(),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> removeKeys({required Id id}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_remove_keys(
        uniffiClonePointer(),
        FfiConverterId.lower(id),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> removeMeltQuote({required String quoteId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_remove_melt_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> removeMint({required MintUrl mintUrl}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_remove_mint(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> removeMintQuote({required String quoteId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_remove_mint_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> removeTransaction({required TransactionId transactionId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_remove_transaction(
        uniffiClonePointer(),
        FfiConverterTransactionId.lower(transactionId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> reserveMeltQuote({
    required String quoteId,
    required String operationId,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_reserve_melt_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> reserveMintQuote({
    required String quoteId,
    required String operationId,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_reserve_mint_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> reserveProofs({
    required List<PublicKey> ys,
    required String operationId,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_reserve_proofs(
        uniffiClonePointer(),
        FfiConverterSequencePublicKey.lower(ys),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> updateMintUrl({
    required MintUrl oldMintUrl,
    required MintUrl newMintUrl,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_update_mint_url(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(oldMintUrl),
        FfiConverterMintUrl.lower(newMintUrl),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> updateProofs({
    required List<ProofInfo> added,
    required List<PublicKey> removedYs,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_update_proofs(
        uniffiClonePointer(),
        FfiConverterSequenceProofInfo.lower(added),
        FfiConverterSequencePublicKey.lower(removedYs),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> updateProofsState({
    required List<PublicKey> ys,
    required ProofState state,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_update_proofs_state(
        uniffiClonePointer(),
        FfiConverterSequencePublicKey.lower(ys),
        FfiConverterProofState.lower(state),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<bool> updateSaga({required String sagaJson}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletpostgresdatabase_update_saga(
        uniffiClonePointer(),
        FfiConverterString.lower(sagaJson),
      ),
      ffi_cdk_ffi_rust_future_poll_i8,
      ffi_cdk_ffi_rust_future_complete_i8,
      ffi_cdk_ffi_rust_future_free_i8,
      FfiConverterBool.lift,
      ffiExceptionErrorHandler,
    );
  }
}

abstract class WalletRepositoryInterface {
  Future<void> createWallet({
    required MintUrl mintUrl,
    required CurrencyUnit? unit,
    required int? targetProofCount,
  });
  Future<Map<WalletKey, Amount>> getBalances();
  Future<Wallet> getWallet({
    required MintUrl mintUrl,
    required CurrencyUnit unit,
  });
  Future<List<Wallet>> getWallets();
  Future<bool> hasMint({required MintUrl mintUrl});
  Future<void> removeWallet({
    required MintUrl mintUrl,
    required CurrencyUnit currencyUnit,
  });
  Future<void> setMetadataCacheTtlForAllMints({required int? ttlSecs});
  Future<void> setMetadataCacheTtlForMint({
    required MintUrl mintUrl,
    required int? ttlSecs,
  });
}

final _WalletRepositoryFinalizer = Finalizer<Pointer<Void>>((ptr) {
  rustCall((status) => uniffi_cdk_ffi_fn_free_walletrepository(ptr, status));
});

class WalletRepository implements WalletRepositoryInterface {
  late final Pointer<Void> _ptr;
  WalletRepository._(this._ptr) {
    _WalletRepositoryFinalizer.attach(this, _ptr, detach: this);
  }
  WalletRepository({required String mnemonic, required WalletStore store})
    : _ptr = rustCall(
        (status) => uniffi_cdk_ffi_fn_constructor_walletrepository_new(
          FfiConverterString.lower(mnemonic),
          FfiConverterWalletStore.lower(store),
          status,
        ),
        ffiExceptionErrorHandler,
      ) {
    _WalletRepositoryFinalizer.attach(this, _ptr, detach: this);
  }
  WalletRepository.newWithProxy({
    required String mnemonic,
    required WalletStore store,
    required String proxyUrl,
  }) : _ptr = rustCall(
         (status) =>
             uniffi_cdk_ffi_fn_constructor_walletrepository_new_with_proxy(
               FfiConverterString.lower(mnemonic),
               FfiConverterWalletStore.lower(store),
               FfiConverterString.lower(proxyUrl),
               status,
             ),
         ffiExceptionErrorHandler,
       ) {
    _WalletRepositoryFinalizer.attach(this, _ptr, detach: this);
  }
  factory WalletRepository.lift(Pointer<Void> ptr) {
    return WalletRepository._(ptr);
  }
  static Pointer<Void> lower(WalletRepository value) {
    return value.uniffiClonePointer();
  }

  Pointer<Void> uniffiClonePointer() {
    return rustCall(
      (status) => uniffi_cdk_ffi_fn_clone_walletrepository(_ptr, status),
    );
  }

  static int allocationSize(WalletRepository value) {
    return 8;
  }

  static LiftRetVal<WalletRepository> read(Uint8List buf) {
    final handle = buf.buffer.asByteData(buf.offsetInBytes).getInt64(0);
    final pointer = Pointer<Void>.fromAddress(handle);
    return LiftRetVal(WalletRepository.lift(pointer), 8);
  }

  static int write(WalletRepository value, Uint8List buf) {
    final handle = lower(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt64(0, handle.address);
    return 8;
  }

  void dispose() {
    _WalletRepositoryFinalizer.detach(this);
    rustCall((status) => uniffi_cdk_ffi_fn_free_walletrepository(_ptr, status));
  }

  Future<void> createWallet({
    required MintUrl mintUrl,
    required CurrencyUnit? unit,
    required int? targetProofCount,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletrepository_create_wallet(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
        FfiConverterOptionalCurrencyUnit.lower(unit),
        FfiConverterOptionalUInt32.lower(targetProofCount),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<Map<WalletKey, Amount>> getBalances() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletrepository_get_balances(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterMapWalletKeyToAmount.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Wallet> getWallet({
    required MintUrl mintUrl,
    required CurrencyUnit unit,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletrepository_get_wallet(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
        FfiConverterCurrencyUnit.lower(unit),
      ),
      ffi_cdk_ffi_rust_future_poll_u64,
      ffi_cdk_ffi_rust_future_complete_u64,
      ffi_cdk_ffi_rust_future_free_u64,
      (ptr) => Wallet.lift(Pointer<Void>.fromAddress(ptr)),
      ffiExceptionErrorHandler,
    );
  }

  Future<List<Wallet>> getWallets() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletrepository_get_wallets(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceWallet.lift,
      null,
    );
  }

  Future<bool> hasMint({required MintUrl mintUrl}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletrepository_has_mint(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
      ),
      ffi_cdk_ffi_rust_future_poll_i8,
      ffi_cdk_ffi_rust_future_complete_i8,
      ffi_cdk_ffi_rust_future_free_i8,
      FfiConverterBool.lift,
      null,
    );
  }

  Future<void> removeWallet({
    required MintUrl mintUrl,
    required CurrencyUnit currencyUnit,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletrepository_remove_wallet(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
        FfiConverterCurrencyUnit.lower(currencyUnit),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> setMetadataCacheTtlForAllMints({required int? ttlSecs}) {
    return uniffiRustCallAsync(
      () =>
          uniffi_cdk_ffi_fn_method_walletrepository_set_metadata_cache_ttl_for_all_mints(
            uniffiClonePointer(),
            FfiConverterOptionalUInt64.lower(ttlSecs),
          ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      null,
    );
  }

  Future<void> setMetadataCacheTtlForMint({
    required MintUrl mintUrl,
    required int? ttlSecs,
  }) {
    return uniffiRustCallAsync(
      () =>
          uniffi_cdk_ffi_fn_method_walletrepository_set_metadata_cache_ttl_for_mint(
            uniffiClonePointer(),
            FfiConverterMintUrl.lower(mintUrl),
            FfiConverterOptionalUInt64.lower(ttlSecs),
          ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }
}

abstract class WalletSqliteDatabaseInterface {
  Future<void> addKeys({required KeySet keyset});
  Future<void> addMeltQuote({required MeltQuote quote});
  Future<void> addMint({required MintUrl mintUrl, required MintInfo? mintInfo});
  Future<void> addMintKeysets({
    required MintUrl mintUrl,
    required List<KeySetInfo> keysets,
  });
  Future<void> addMintQuote({required MintQuote quote});
  Future<void> addSaga({required String sagaJson});
  Future<void> addTransaction({required Transaction transaction});
  Future<void> deleteSaga({required String id});
  Future<int> getBalance({
    required MintUrl? mintUrl,
    required CurrencyUnit? unit,
    required List<ProofState>? state,
  });
  Future<List<String>> getIncompleteSagas();
  Future<Keys?> getKeys({required Id id});
  Future<KeySetInfo?> getKeysetById({required Id keysetId});
  Future<MeltQuote?> getMeltQuote({required String quoteId});
  Future<List<MeltQuote>> getMeltQuotes();
  Future<MintInfo?> getMint({required MintUrl mintUrl});
  Future<List<KeySetInfo>?> getMintKeysets({required MintUrl mintUrl});
  Future<MintQuote?> getMintQuote({required String quoteId});
  Future<List<MintQuote>> getMintQuotes();
  Future<Map<MintUrl, MintInfo?>> getMints();
  Future<List<ProofInfo>> getProofs({
    required MintUrl? mintUrl,
    required CurrencyUnit? unit,
    required List<ProofState>? state,
    required List<SpendingConditions>? spendingConditions,
  });
  Future<List<ProofInfo>> getProofsByYs({required List<PublicKey> ys});
  Future<List<ProofInfo>> getReservedProofs({required String operationId});
  Future<String?> getSaga({required String id});
  Future<Transaction?> getTransaction({required TransactionId transactionId});
  Future<List<MintQuote>> getUnissuedMintQuotes();
  Future<int> incrementKeysetCounter({
    required Id keysetId,
    required int count,
  });
  Future<List<String>> kvList({
    required String primaryNamespace,
    required String secondaryNamespace,
  });
  Future<Uint8List?> kvRead({
    required String primaryNamespace,
    required String secondaryNamespace,
    required String key,
  });
  Future<void> kvRemove({
    required String primaryNamespace,
    required String secondaryNamespace,
    required String key,
  });
  Future<void> kvWrite({
    required String primaryNamespace,
    required String secondaryNamespace,
    required String key,
    required Uint8List value,
  });
  Future<List<Transaction>> listTransactions({
    required MintUrl? mintUrl,
    required TransactionDirection? direction,
    required CurrencyUnit? unit,
  });
  Future<void> releaseMeltQuote({required String operationId});
  Future<void> releaseMintQuote({required String operationId});
  Future<void> releaseProofs({required String operationId});
  Future<void> removeKeys({required Id id});
  Future<void> removeMeltQuote({required String quoteId});
  Future<void> removeMint({required MintUrl mintUrl});
  Future<void> removeMintQuote({required String quoteId});
  Future<void> removeTransaction({required TransactionId transactionId});
  Future<void> reserveMeltQuote({
    required String quoteId,
    required String operationId,
  });
  Future<void> reserveMintQuote({
    required String quoteId,
    required String operationId,
  });
  Future<void> reserveProofs({
    required List<PublicKey> ys,
    required String operationId,
  });
  Future<void> updateMintUrl({
    required MintUrl oldMintUrl,
    required MintUrl newMintUrl,
  });
  Future<void> updateProofs({
    required List<ProofInfo> added,
    required List<PublicKey> removedYs,
  });
  Future<void> updateProofsState({
    required List<PublicKey> ys,
    required ProofState state,
  });
  Future<bool> updateSaga({required String sagaJson});
}

final _WalletSqliteDatabaseFinalizer = Finalizer<Pointer<Void>>((ptr) {
  rustCall(
    (status) => uniffi_cdk_ffi_fn_free_walletsqlitedatabase(ptr, status),
  );
});

class WalletSqliteDatabase implements WalletSqliteDatabaseInterface {
  late final Pointer<Void> _ptr;
  WalletSqliteDatabase._(this._ptr) {
    _WalletSqliteDatabaseFinalizer.attach(this, _ptr, detach: this);
  }
  WalletSqliteDatabase({required String filePath})
    : _ptr = rustCall(
        (status) => uniffi_cdk_ffi_fn_constructor_walletsqlitedatabase_new(
          FfiConverterString.lower(filePath),
          status,
        ),
        ffiExceptionErrorHandler,
      ) {
    _WalletSqliteDatabaseFinalizer.attach(this, _ptr, detach: this);
  }
  WalletSqliteDatabase.newInMemory()
    : _ptr = rustCall(
        (status) =>
            uniffi_cdk_ffi_fn_constructor_walletsqlitedatabase_new_in_memory(
              status,
            ),
        ffiExceptionErrorHandler,
      ) {
    _WalletSqliteDatabaseFinalizer.attach(this, _ptr, detach: this);
  }
  factory WalletSqliteDatabase.lift(Pointer<Void> ptr) {
    return WalletSqliteDatabase._(ptr);
  }
  static Pointer<Void> lower(WalletSqliteDatabase value) {
    return value.uniffiClonePointer();
  }

  Pointer<Void> uniffiClonePointer() {
    return rustCall(
      (status) => uniffi_cdk_ffi_fn_clone_walletsqlitedatabase(_ptr, status),
    );
  }

  static int allocationSize(WalletSqliteDatabase value) {
    return 8;
  }

  static LiftRetVal<WalletSqliteDatabase> read(Uint8List buf) {
    final handle = buf.buffer.asByteData(buf.offsetInBytes).getInt64(0);
    final pointer = Pointer<Void>.fromAddress(handle);
    return LiftRetVal(WalletSqliteDatabase.lift(pointer), 8);
  }

  static int write(WalletSqliteDatabase value, Uint8List buf) {
    final handle = lower(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt64(0, handle.address);
    return 8;
  }

  void dispose() {
    _WalletSqliteDatabaseFinalizer.detach(this);
    rustCall(
      (status) => uniffi_cdk_ffi_fn_free_walletsqlitedatabase(_ptr, status),
    );
  }

  Future<void> addKeys({required KeySet keyset}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_keys(
        uniffiClonePointer(),
        FfiConverterKeySet.lower(keyset),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> addMeltQuote({required MeltQuote quote}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_melt_quote(
        uniffiClonePointer(),
        FfiConverterMeltQuote.lower(quote),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> addMint({
    required MintUrl mintUrl,
    required MintInfo? mintInfo,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_mint(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
        FfiConverterOptionalMintInfo.lower(mintInfo),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> addMintKeysets({
    required MintUrl mintUrl,
    required List<KeySetInfo> keysets,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_mint_keysets(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
        FfiConverterSequenceKeySetInfo.lower(keysets),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> addMintQuote({required MintQuote quote}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_mint_quote(
        uniffiClonePointer(),
        FfiConverterMintQuote.lower(quote),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> addSaga({required String sagaJson}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_saga(
        uniffiClonePointer(),
        FfiConverterString.lower(sagaJson),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> addTransaction({required Transaction transaction}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_transaction(
        uniffiClonePointer(),
        FfiConverterTransaction.lower(transaction),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> deleteSaga({required String id}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_delete_saga(
        uniffiClonePointer(),
        FfiConverterString.lower(id),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<int> getBalance({
    required MintUrl? mintUrl,
    required CurrencyUnit? unit,
    required List<ProofState>? state,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_balance(
        uniffiClonePointer(),
        FfiConverterOptionalMintUrl.lower(mintUrl),
        FfiConverterOptionalCurrencyUnit.lower(unit),
        FfiConverterOptionalSequenceProofState.lower(state),
      ),
      ffi_cdk_ffi_rust_future_poll_u64,
      ffi_cdk_ffi_rust_future_complete_u64,
      ffi_cdk_ffi_rust_future_free_u64,
      FfiConverterUInt64.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<String>> getIncompleteSagas() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_incomplete_sagas(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceString.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Keys?> getKeys({required Id id}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_keys(
        uniffiClonePointer(),
        FfiConverterId.lower(id),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalKeys.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<KeySetInfo?> getKeysetById({required Id keysetId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_keyset_by_id(
        uniffiClonePointer(),
        FfiConverterId.lower(keysetId),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalKeySetInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MeltQuote?> getMeltQuote({required String quoteId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_melt_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalMeltQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<MeltQuote>> getMeltQuotes() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_melt_quotes(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceMeltQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MintInfo?> getMint({required MintUrl mintUrl}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_mint(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalMintInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<KeySetInfo>?> getMintKeysets({required MintUrl mintUrl}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_mint_keysets(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalSequenceKeySetInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<MintQuote?> getMintQuote({required String quoteId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_mint_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalMintQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<MintQuote>> getMintQuotes() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_mint_quotes(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceMintQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Map<MintUrl, MintInfo?>> getMints() {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_mints(
        uniffiClonePointer(),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterMapMintUrlToOptionalMintInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<ProofInfo>> getProofs({
    required MintUrl? mintUrl,
    required CurrencyUnit? unit,
    required List<ProofState>? state,
    required List<SpendingConditions>? spendingConditions,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_proofs(
        uniffiClonePointer(),
        FfiConverterOptionalMintUrl.lower(mintUrl),
        FfiConverterOptionalCurrencyUnit.lower(unit),
        FfiConverterOptionalSequenceProofState.lower(state),
        FfiConverterOptionalSequenceSpendingConditions.lower(
          spendingConditions,
        ),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceProofInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<ProofInfo>> getProofsByYs({required List<PublicKey> ys}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_proofs_by_ys(
        uniffiClonePointer(),
        FfiConverterSequencePublicKey.lower(ys),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceProofInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<ProofInfo>> getReservedProofs({required String operationId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_reserved_proofs(
        uniffiClonePointer(),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceProofInfo.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<String?> getSaga({required String id}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_saga(
        uniffiClonePointer(),
        FfiConverterString.lower(id),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalString.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Transaction?> getTransaction({required TransactionId transactionId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_transaction(
        uniffiClonePointer(),
        FfiConverterTransactionId.lower(transactionId),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalTransaction.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<MintQuote>> getUnissuedMintQuotes() {
    return uniffiRustCallAsync(
      () =>
          uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_unissued_mint_quotes(
            uniffiClonePointer(),
          ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceMintQuote.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<int> incrementKeysetCounter({
    required Id keysetId,
    required int count,
  }) {
    return uniffiRustCallAsync(
      () =>
          uniffi_cdk_ffi_fn_method_walletsqlitedatabase_increment_keyset_counter(
            uniffiClonePointer(),
            FfiConverterId.lower(keysetId),
            FfiConverterUInt32.lower(count),
          ),
      ffi_cdk_ffi_rust_future_poll_u32,
      ffi_cdk_ffi_rust_future_complete_u32,
      ffi_cdk_ffi_rust_future_free_u32,
      FfiConverterUInt32.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<List<String>> kvList({
    required String primaryNamespace,
    required String secondaryNamespace,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_kv_list(
        uniffiClonePointer(),
        FfiConverterString.lower(primaryNamespace),
        FfiConverterString.lower(secondaryNamespace),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceString.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<Uint8List?> kvRead({
    required String primaryNamespace,
    required String secondaryNamespace,
    required String key,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_kv_read(
        uniffiClonePointer(),
        FfiConverterString.lower(primaryNamespace),
        FfiConverterString.lower(secondaryNamespace),
        FfiConverterString.lower(key),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterOptionalUint8List.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<void> kvRemove({
    required String primaryNamespace,
    required String secondaryNamespace,
    required String key,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_kv_remove(
        uniffiClonePointer(),
        FfiConverterString.lower(primaryNamespace),
        FfiConverterString.lower(secondaryNamespace),
        FfiConverterString.lower(key),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> kvWrite({
    required String primaryNamespace,
    required String secondaryNamespace,
    required String key,
    required Uint8List value,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_kv_write(
        uniffiClonePointer(),
        FfiConverterString.lower(primaryNamespace),
        FfiConverterString.lower(secondaryNamespace),
        FfiConverterString.lower(key),
        FfiConverterUint8List.lower(value),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<List<Transaction>> listTransactions({
    required MintUrl? mintUrl,
    required TransactionDirection? direction,
    required CurrencyUnit? unit,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_list_transactions(
        uniffiClonePointer(),
        FfiConverterOptionalMintUrl.lower(mintUrl),
        FfiConverterOptionalTransactionDirection.lower(direction),
        FfiConverterOptionalCurrencyUnit.lower(unit),
      ),
      ffi_cdk_ffi_rust_future_poll_rust_buffer,
      ffi_cdk_ffi_rust_future_complete_rust_buffer,
      ffi_cdk_ffi_rust_future_free_rust_buffer,
      FfiConverterSequenceTransaction.lift,
      ffiExceptionErrorHandler,
    );
  }

  Future<void> releaseMeltQuote({required String operationId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_release_melt_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> releaseMintQuote({required String operationId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_release_mint_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> releaseProofs({required String operationId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_release_proofs(
        uniffiClonePointer(),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> removeKeys({required Id id}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_remove_keys(
        uniffiClonePointer(),
        FfiConverterId.lower(id),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> removeMeltQuote({required String quoteId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_remove_melt_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> removeMint({required MintUrl mintUrl}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_remove_mint(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(mintUrl),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> removeMintQuote({required String quoteId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_remove_mint_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> removeTransaction({required TransactionId transactionId}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_remove_transaction(
        uniffiClonePointer(),
        FfiConverterTransactionId.lower(transactionId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> reserveMeltQuote({
    required String quoteId,
    required String operationId,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_reserve_melt_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> reserveMintQuote({
    required String quoteId,
    required String operationId,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_reserve_mint_quote(
        uniffiClonePointer(),
        FfiConverterString.lower(quoteId),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> reserveProofs({
    required List<PublicKey> ys,
    required String operationId,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_reserve_proofs(
        uniffiClonePointer(),
        FfiConverterSequencePublicKey.lower(ys),
        FfiConverterString.lower(operationId),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> updateMintUrl({
    required MintUrl oldMintUrl,
    required MintUrl newMintUrl,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_update_mint_url(
        uniffiClonePointer(),
        FfiConverterMintUrl.lower(oldMintUrl),
        FfiConverterMintUrl.lower(newMintUrl),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> updateProofs({
    required List<ProofInfo> added,
    required List<PublicKey> removedYs,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_update_proofs(
        uniffiClonePointer(),
        FfiConverterSequenceProofInfo.lower(added),
        FfiConverterSequencePublicKey.lower(removedYs),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<void> updateProofsState({
    required List<PublicKey> ys,
    required ProofState state,
  }) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_update_proofs_state(
        uniffiClonePointer(),
        FfiConverterSequencePublicKey.lower(ys),
        FfiConverterProofState.lower(state),
      ),
      ffi_cdk_ffi_rust_future_poll_void,
      ffi_cdk_ffi_rust_future_complete_void,
      ffi_cdk_ffi_rust_future_free_void,
      (_) {},
      ffiExceptionErrorHandler,
    );
  }

  Future<bool> updateSaga({required String sagaJson}) {
    return uniffiRustCallAsync(
      () => uniffi_cdk_ffi_fn_method_walletsqlitedatabase_update_saga(
        uniffiClonePointer(),
        FfiConverterString.lower(sagaJson),
      ),
      ffi_cdk_ffi_rust_future_poll_i8,
      ffi_cdk_ffi_rust_future_complete_i8,
      ffi_cdk_ffi_rust_future_free_i8,
      FfiConverterBool.lift,
      ffiExceptionErrorHandler,
    );
  }
}

class UniffiInternalError implements Exception {
  static const int bufferOverflow = 0;
  static const int incompleteData = 1;
  static const int unexpectedOptionalTag = 2;
  static const int unexpectedEnumCase = 3;
  static const int unexpectedNullPointer = 4;
  static const int unexpectedRustCallStatusCode = 5;
  static const int unexpectedRustCallError = 6;
  static const int unexpectedStaleHandle = 7;
  static const int rustPanic = 8;
  final int errorCode;
  final String? panicMessage;
  const UniffiInternalError(this.errorCode, this.panicMessage);
  static UniffiInternalError panicked(String message) {
    return UniffiInternalError(rustPanic, message);
  }

  @override
  String toString() {
    switch (errorCode) {
      case bufferOverflow:
        return "UniFfi::BufferOverflow";
      case incompleteData:
        return "UniFfi::IncompleteData";
      case unexpectedOptionalTag:
        return "UniFfi::UnexpectedOptionalTag";
      case unexpectedEnumCase:
        return "UniFfi::UnexpectedEnumCase";
      case unexpectedNullPointer:
        return "UniFfi::UnexpectedNullPointer";
      case unexpectedRustCallStatusCode:
        return "UniFfi::UnexpectedRustCallStatusCode";
      case unexpectedRustCallError:
        return "UniFfi::UnexpectedRustCallError";
      case unexpectedStaleHandle:
        return "UniFfi::UnexpectedStaleHandle";
      case rustPanic:
        return "UniFfi::rustPanic: $panicMessage";
      default:
        return "UniFfi::UnknownError: $errorCode";
    }
  }
}

const int CALL_SUCCESS = 0;
const int CALL_ERROR = 1;
const int CALL_UNEXPECTED_ERROR = 2;

final class RustCallStatus extends Struct {
  @Int8()
  external int code;
  external RustBuffer errorBuf;
}

void checkCallStatus(
  UniffiRustCallStatusErrorHandler errorHandler,
  Pointer<RustCallStatus> status,
) {
  if (status.ref.code == CALL_SUCCESS) {
    return;
  } else if (status.ref.code == CALL_ERROR) {
    throw errorHandler.lift(status.ref.errorBuf);
  } else if (status.ref.code == CALL_UNEXPECTED_ERROR) {
    if (status.ref.errorBuf.len > 0) {
      throw UniffiInternalError.panicked(
        FfiConverterString.lift(status.ref.errorBuf),
      );
    } else {
      throw UniffiInternalError.panicked("Rust panic");
    }
  } else {
    throw UniffiInternalError.panicked(
      "Unexpected RustCallStatus code: \${status.ref.code}",
    );
  }
}

T rustCall<T>(
  T Function(Pointer<RustCallStatus>) callback, [
  UniffiRustCallStatusErrorHandler? errorHandler,
]) {
  final status = calloc<RustCallStatus>();
  try {
    final result = callback(status);
    checkCallStatus(errorHandler ?? NullRustCallStatusErrorHandler(), status);
    return result;
  } finally {
    calloc.free(status);
  }
}

T rustCallWithLifter<T, F>(
  F Function(Pointer<RustCallStatus>) ffiCall,
  T Function(F) lifter, [
  UniffiRustCallStatusErrorHandler? errorHandler,
]) {
  final status = calloc<RustCallStatus>();
  try {
    final rawResult = ffiCall(status);
    checkCallStatus(errorHandler ?? NullRustCallStatusErrorHandler(), status);
    return lifter(rawResult);
  } finally {
    calloc.free(status);
  }
}

class NullRustCallStatusErrorHandler extends UniffiRustCallStatusErrorHandler {
  @override
  Exception lift(RustBuffer errorBuf) {
    errorBuf.free();
    return UniffiInternalError.panicked("Unexpected CALL_ERROR");
  }
}

abstract class UniffiRustCallStatusErrorHandler {
  Exception lift(RustBuffer errorBuf);
}

final class RustBuffer extends Struct {
  @Uint64()
  external int capacity;
  @Uint64()
  external int len;
  external Pointer<Uint8> data;
  static RustBuffer alloc(int size) {
    return rustCall((status) => ffi_cdk_ffi_rustbuffer_alloc(size, status));
  }

  static RustBuffer fromBytes(ForeignBytes bytes) {
    return rustCall(
      (status) => ffi_cdk_ffi_rustbuffer_from_bytes(bytes, status),
    );
  }

  void free() {
    rustCall((status) => ffi_cdk_ffi_rustbuffer_free(this, status));
  }

  RustBuffer reserve(int additionalCapacity) {
    return rustCall(
      (status) =>
          ffi_cdk_ffi_rustbuffer_reserve(this, additionalCapacity, status),
    );
  }

  Uint8List asUint8List() {
    final dataList = data.asTypedList(len);
    final byteData = ByteData.sublistView(dataList);
    return Uint8List.view(byteData.buffer);
  }

  @override
  String toString() {
    return "RustBuffer{capacity: \$capacity, len: \$len, data: \$data}";
  }
}

RustBuffer toRustBuffer(Uint8List data) {
  final length = data.length;
  final Pointer<Uint8> frameData = calloc<Uint8>(length);
  final pointerList = frameData.asTypedList(length);
  pointerList.setAll(0, data);
  final bytes = calloc<ForeignBytes>();
  bytes.ref.len = length;
  bytes.ref.data = frameData;
  return RustBuffer.fromBytes(bytes.ref);
}

final class ForeignBytes extends Struct {
  @Int32()
  external int len;
  external Pointer<Uint8> data;
  void free() {
    calloc.free(data);
  }
}

class LiftRetVal<T> {
  final T value;
  final int bytesRead;
  const LiftRetVal(this.value, this.bytesRead);
  LiftRetVal<T> copyWithOffset(int offset) {
    return LiftRetVal(value, bytesRead + offset);
  }
}

abstract class FfiConverter<D, F> {
  const FfiConverter();
  D lift(F value);
  F lower(D value);
  D read(ByteData buffer, int offset);
  void write(D value, ByteData buffer, int offset);
  int size(D value);
}

mixin FfiConverterPrimitive<T> on FfiConverter<T, T> {
  @override
  T lift(T value) => value;
  @override
  T lower(T value) => value;
}
Uint8List createUint8ListFromInt(int value) {
  int length = value.bitLength ~/ 8 + 1;
  if (length != 4 && length != 8) {
    length = (value < 0x100000000) ? 4 : 8;
  }
  Uint8List uint8List = Uint8List(length);
  for (int i = length - 1; i >= 0; i--) {
    uint8List[i] = value & 0xFF;
    value >>= 8;
  }
  return uint8List;
}

class FfiConverterOptionalSequenceKeySetInfo {
  static List<KeySetInfo>? lift(RustBuffer buf) {
    return FfiConverterOptionalSequenceKeySetInfo.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<KeySetInfo>?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterSequenceKeySetInfo.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<List<KeySetInfo>?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([List<KeySetInfo>? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterSequenceKeySetInfo.allocationSize(value) + 1;
  }

  static RustBuffer lower(List<KeySetInfo>? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalSequenceKeySetInfo.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalSequenceKeySetInfo.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(List<KeySetInfo>? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterSequenceKeySetInfo.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalSendMemo {
  static SendMemo? lift(RustBuffer buf) {
    return FfiConverterOptionalSendMemo.read(buf.asUint8List()).value;
  }

  static LiftRetVal<SendMemo?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterSendMemo.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<SendMemo?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([SendMemo? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterSendMemo.allocationSize(value) + 1;
  }

  static RustBuffer lower(SendMemo? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalSendMemo.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalSendMemo.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(SendMemo? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterSendMemo.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalWitness {
  static Witness? lift(RustBuffer buf) {
    return FfiConverterOptionalWitness.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Witness?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterWitness.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<Witness?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([Witness? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterWitness.allocationSize(value) + 1;
  }

  static RustBuffer lower(Witness? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalWitness.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalWitness.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(Witness? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterWitness.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterSequenceProofStateUpdate {
  static List<ProofStateUpdate> lift(RustBuffer buf) {
    return FfiConverterSequenceProofStateUpdate.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<ProofStateUpdate>> read(Uint8List buf) {
    List<ProofStateUpdate> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterProofStateUpdate.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<ProofStateUpdate> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterProofStateUpdate.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<ProofStateUpdate> value) {
    return value
            .map((l) => FfiConverterProofStateUpdate.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<ProofStateUpdate> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterBool {
  static bool lift(int value) {
    return value == 1;
  }

  static int lower(bool value) {
    return value ? 1 : 0;
  }

  static LiftRetVal<bool> read(Uint8List buf) {
    return LiftRetVal(FfiConverterBool.lift(buf.first), 1);
  }

  static RustBuffer lowerIntoRustBuffer(bool value) {
    return toRustBuffer(Uint8List.fromList([FfiConverterBool.lower(value)]));
  }

  static int allocationSize([bool value = false]) {
    return 1;
  }

  static int write(bool value, Uint8List buf) {
    buf.setAll(0, [value ? 1 : 0]);
    return allocationSize();
  }
}

class FfiConverterUInt8 {
  static int lift(int value) => value;
  static LiftRetVal<int> read(Uint8List buf) {
    return LiftRetVal(buf.buffer.asByteData(buf.offsetInBytes).getUint8(0), 1);
  }

  static int lower(int value) {
    if (value < 0 || value > 255) {
      throw ArgumentError("Value out of range for u8: " + value.toString());
    }
    return value;
  }

  static int allocationSize([int value = 0]) {
    return 1;
  }

  static int write(int value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setUint8(0, lower(value));
    return 1;
  }
}

class FfiConverterSequenceString {
  static List<String> lift(RustBuffer buf) {
    return FfiConverterSequenceString.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<String>> read(Uint8List buf) {
    List<String> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterString.read(Uint8List.view(buf.buffer, offset));
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<String> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterString.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<String> value) {
    return value
            .map((l) => FfiConverterString.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<String> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterSequenceTransport {
  static List<Transport> lift(RustBuffer buf) {
    return FfiConverterSequenceTransport.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<Transport>> read(Uint8List buf) {
    List<Transport> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterTransport.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<Transport> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterTransport.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<Transport> value) {
    return value
            .map((l) => FfiConverterTransport.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<Transport> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterSequenceUInt64 {
  static List<int> lift(RustBuffer buf) {
    return FfiConverterSequenceUInt64.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<int>> read(Uint8List buf) {
    List<int> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterUInt64.read(Uint8List.view(buf.buffer, offset));
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<int> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterUInt64.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<int> value) {
    return value
            .map((l) => FfiConverterUInt64.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<int> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalTransactionDirection {
  static TransactionDirection? lift(RustBuffer buf) {
    return FfiConverterOptionalTransactionDirection.read(
      buf.asUint8List(),
    ).value;
  }

  static LiftRetVal<TransactionDirection?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterTransactionDirection.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<TransactionDirection?>(
      result.value,
      result.bytesRead + 1,
    );
  }

  static int allocationSize([TransactionDirection? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterTransactionDirection.allocationSize(value) + 1;
  }

  static RustBuffer lower(TransactionDirection? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalTransactionDirection.allocationSize(
      value,
    );
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalTransactionDirection.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(TransactionDirection? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterTransactionDirection.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterMapMintUrlToOptionalMintInfo {
  static Map<MintUrl, MintInfo?> lift(RustBuffer buf) {
    return FfiConverterMapMintUrlToOptionalMintInfo.read(
      buf.asUint8List(),
    ).value;
  }

  static LiftRetVal<Map<MintUrl, MintInfo?>> read(Uint8List buf) {
    final map = <MintUrl, MintInfo?>{};
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final k = FfiConverterMintUrl.read(Uint8List.view(buf.buffer, offset));
      offset += k.bytesRead;
      final v = FfiConverterOptionalMintInfo.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += v.bytesRead;
      map[k.value] = v.value;
    }
    return LiftRetVal(map, offset - buf.offsetInBytes);
  }

  static int write(Map<MintUrl, MintInfo?> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (final entry in value.entries) {
      offset += FfiConverterMintUrl.write(
        entry.key,
        Uint8List.view(buf.buffer, offset),
      );
      offset += FfiConverterOptionalMintInfo.write(
        entry.value,
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(Map<MintUrl, MintInfo?> value) {
    return value.entries
        .map(
          (e) =>
              FfiConverterMintUrl.allocationSize(e.key) +
              FfiConverterOptionalMintInfo.allocationSize(e.value),
        )
        .fold(4, (a, b) => a + b);
  }

  static RustBuffer lower(Map<MintUrl, MintInfo?> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalSequenceProof {
  static List<Proof>? lift(RustBuffer buf) {
    return FfiConverterOptionalSequenceProof.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<Proof>?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterSequenceProof.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<List<Proof>?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([List<Proof>? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterSequenceProof.allocationSize(value) + 1;
  }

  static RustBuffer lower(List<Proof>? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalSequenceProof.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalSequenceProof.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(List<Proof>? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterSequenceProof.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalMintUrl {
  static MintUrl? lift(RustBuffer buf) {
    return FfiConverterOptionalMintUrl.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MintUrl?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterMintUrl.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<MintUrl?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([MintUrl? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterMintUrl.allocationSize(value) + 1;
  }

  static RustBuffer lower(MintUrl? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalMintUrl.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalMintUrl.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(MintUrl? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterMintUrl.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalSequenceString {
  static List<String>? lift(RustBuffer buf) {
    return FfiConverterOptionalSequenceString.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<String>?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterSequenceString.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<List<String>?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([List<String>? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterSequenceString.allocationSize(value) + 1;
  }

  static RustBuffer lower(List<String>? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalSequenceString.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalSequenceString.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(List<String>? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterSequenceString.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterUInt64 {
  static int lift(int value) => value;
  static LiftRetVal<int> read(Uint8List buf) {
    return LiftRetVal(buf.buffer.asByteData(buf.offsetInBytes).getUint64(0), 8);
  }

  static int lower(int value) {
    if (value < 0) {
      throw ArgumentError("Value out of range for u64: " + value.toString());
    }
    return value;
  }

  static int allocationSize([int value = 0]) {
    return 8;
  }

  static int write(int value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setUint64(0, lower(value));
    return 8;
  }
}

class FfiConverterOptionalBool {
  static bool? lift(RustBuffer buf) {
    return FfiConverterOptionalBool.read(buf.asUint8List()).value;
  }

  static LiftRetVal<bool?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterBool.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<bool?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([bool? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterBool.allocationSize(value) + 1;
  }

  static RustBuffer lower(bool? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalBool.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalBool.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(bool? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterBool.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterSequenceTransaction {
  static List<Transaction> lift(RustBuffer buf) {
    return FfiConverterSequenceTransaction.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<Transaction>> read(Uint8List buf) {
    List<Transaction> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterTransaction.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<Transaction> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterTransaction.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<Transaction> value) {
    return value
            .map((l) => FfiConverterTransaction.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<Transaction> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalUInt64 {
  static int? lift(RustBuffer buf) {
    return FfiConverterOptionalUInt64.read(buf.asUint8List()).value;
  }

  static LiftRetVal<int?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterUInt64.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<int?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([int? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterUInt64.allocationSize(value) + 1;
  }

  static RustBuffer lower(int? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalUInt64.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalUInt64.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(int? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterUInt64.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalKeySetInfo {
  static KeySetInfo? lift(RustBuffer buf) {
    return FfiConverterOptionalKeySetInfo.read(buf.asUint8List()).value;
  }

  static LiftRetVal<KeySetInfo?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterKeySetInfo.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<KeySetInfo?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([KeySetInfo? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterKeySetInfo.allocationSize(value) + 1;
  }

  static RustBuffer lower(KeySetInfo? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalKeySetInfo.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalKeySetInfo.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(KeySetInfo? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterKeySetInfo.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterSequenceWallet {
  static List<Wallet> lift(RustBuffer buf) {
    return FfiConverterSequenceWallet.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<Wallet>> read(Uint8List buf) {
    List<Wallet> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = Wallet.read(Uint8List.view(buf.buffer, offset));
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<Wallet> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += Wallet.write(value[i], Uint8List.view(buf.buffer, offset));
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<Wallet> value) {
    return value.map((l) => Wallet.allocationSize(l)).fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<Wallet> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterSequenceProof {
  static List<Proof> lift(RustBuffer buf) {
    return FfiConverterSequenceProof.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<Proof>> read(Uint8List buf) {
    List<Proof> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterProof.read(Uint8List.view(buf.buffer, offset));
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<Proof> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterProof.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<Proof> value) {
    return value
            .map((l) => FfiConverterProof.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<Proof> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalSequenceProofState {
  static List<ProofState>? lift(RustBuffer buf) {
    return FfiConverterOptionalSequenceProofState.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<ProofState>?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterSequenceProofState.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<List<ProofState>?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([List<ProofState>? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterSequenceProofState.allocationSize(value) + 1;
  }

  static RustBuffer lower(List<ProofState>? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalSequenceProofState.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalSequenceProofState.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(List<ProofState>? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterSequenceProofState.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterMapWalletKeyToAmount {
  static Map<WalletKey, Amount> lift(RustBuffer buf) {
    return FfiConverterMapWalletKeyToAmount.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Map<WalletKey, Amount>> read(Uint8List buf) {
    final map = <WalletKey, Amount>{};
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final k = FfiConverterWalletKey.read(Uint8List.view(buf.buffer, offset));
      offset += k.bytesRead;
      final v = FfiConverterAmount.read(Uint8List.view(buf.buffer, offset));
      offset += v.bytesRead;
      map[k.value] = v.value;
    }
    return LiftRetVal(map, offset - buf.offsetInBytes);
  }

  static int write(Map<WalletKey, Amount> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (final entry in value.entries) {
      offset += FfiConverterWalletKey.write(
        entry.key,
        Uint8List.view(buf.buffer, offset),
      );
      offset += FfiConverterAmount.write(
        entry.value,
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(Map<WalletKey, Amount> value) {
    return value.entries
        .map(
          (e) =>
              FfiConverterWalletKey.allocationSize(e.key) +
              FfiConverterAmount.allocationSize(e.value),
        )
        .fold(4, (a, b) => a + b);
  }

  static RustBuffer lower(Map<WalletKey, Amount> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterSequenceMintMethodSettings {
  static List<MintMethodSettings> lift(RustBuffer buf) {
    return FfiConverterSequenceMintMethodSettings.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<MintMethodSettings>> read(Uint8List buf) {
    List<MintMethodSettings> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterMintMethodSettings.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<MintMethodSettings> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterMintMethodSettings.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<MintMethodSettings> value) {
    return value
            .map((l) => FfiConverterMintMethodSettings.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<MintMethodSettings> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterSequenceProofInfo {
  static List<ProofInfo> lift(RustBuffer buf) {
    return FfiConverterSequenceProofInfo.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<ProofInfo>> read(Uint8List buf) {
    List<ProofInfo> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterProofInfo.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<ProofInfo> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterProofInfo.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<ProofInfo> value) {
    return value
            .map((l) => FfiConverterProofInfo.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<ProofInfo> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterSequenceCurrencyUnit {
  static List<CurrencyUnit> lift(RustBuffer buf) {
    return FfiConverterSequenceCurrencyUnit.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<CurrencyUnit>> read(Uint8List buf) {
    List<CurrencyUnit> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterCurrencyUnit.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<CurrencyUnit> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterCurrencyUnit.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<CurrencyUnit> value) {
    return value
            .map((l) => FfiConverterCurrencyUnit.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<CurrencyUnit> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterMapStringToString {
  static Map<String, String> lift(RustBuffer buf) {
    return FfiConverterMapStringToString.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Map<String, String>> read(Uint8List buf) {
    final map = <String, String>{};
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final k = FfiConverterString.read(Uint8List.view(buf.buffer, offset));
      offset += k.bytesRead;
      final v = FfiConverterString.read(Uint8List.view(buf.buffer, offset));
      offset += v.bytesRead;
      map[k.value] = v.value;
    }
    return LiftRetVal(map, offset - buf.offsetInBytes);
  }

  static int write(Map<String, String> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (final entry in value.entries) {
      offset += FfiConverterString.write(
        entry.key,
        Uint8List.view(buf.buffer, offset),
      );
      offset += FfiConverterString.write(
        entry.value,
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(Map<String, String> value) {
    return value.entries
        .map(
          (e) =>
              FfiConverterString.allocationSize(e.key) +
              FfiConverterString.allocationSize(e.value),
        )
        .fold(4, (a, b) => a + b);
  }

  static RustBuffer lower(Map<String, String> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterMapUInt64ToString {
  static Map<int, String> lift(RustBuffer buf) {
    return FfiConverterMapUInt64ToString.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Map<int, String>> read(Uint8List buf) {
    final map = <int, String>{};
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final k = FfiConverterUInt64.read(Uint8List.view(buf.buffer, offset));
      offset += k.bytesRead;
      final v = FfiConverterString.read(Uint8List.view(buf.buffer, offset));
      offset += v.bytesRead;
      map[k.value] = v.value;
    }
    return LiftRetVal(map, offset - buf.offsetInBytes);
  }

  static int write(Map<int, String> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (final entry in value.entries) {
      offset += FfiConverterUInt64.write(
        entry.key,
        Uint8List.view(buf.buffer, offset),
      );
      offset += FfiConverterString.write(
        entry.value,
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(Map<int, String> value) {
    return value.entries
        .map(
          (e) =>
              FfiConverterUInt64.allocationSize(e.key) +
              FfiConverterString.allocationSize(e.value),
        )
        .fold(4, (a, b) => a + b);
  }

  static RustBuffer lower(Map<int, String> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalConditions {
  static Conditions? lift(RustBuffer buf) {
    return FfiConverterOptionalConditions.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Conditions?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterConditions.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<Conditions?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([Conditions? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterConditions.allocationSize(value) + 1;
  }

  static RustBuffer lower(Conditions? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalConditions.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalConditions.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(Conditions? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterConditions.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterSequenceNpubCashQuote {
  static List<NpubCashQuote> lift(RustBuffer buf) {
    return FfiConverterSequenceNpubCashQuote.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<NpubCashQuote>> read(Uint8List buf) {
    List<NpubCashQuote> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterNpubCashQuote.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<NpubCashQuote> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterNpubCashQuote.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<NpubCashQuote> value) {
    return value
            .map((l) => FfiConverterNpubCashQuote.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<NpubCashQuote> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalCurrencyUnit {
  static CurrencyUnit? lift(RustBuffer buf) {
    return FfiConverterOptionalCurrencyUnit.read(buf.asUint8List()).value;
  }

  static LiftRetVal<CurrencyUnit?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterCurrencyUnit.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<CurrencyUnit?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([CurrencyUnit? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterCurrencyUnit.allocationSize(value) + 1;
  }

  static RustBuffer lower(CurrencyUnit? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalCurrencyUnit.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalCurrencyUnit.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(CurrencyUnit? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterCurrencyUnit.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalString {
  static String? lift(RustBuffer buf) {
    return FfiConverterOptionalString.read(buf.asUint8List()).value;
  }

  static LiftRetVal<String?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterString.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<String?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([String? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterString.allocationSize(value) + 1;
  }

  static RustBuffer lower(String? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalString.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalString.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(String? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterString.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalSequenceSpendingConditions {
  static List<SpendingConditions>? lift(RustBuffer buf) {
    return FfiConverterOptionalSequenceSpendingConditions.read(
      buf.asUint8List(),
    ).value;
  }

  static LiftRetVal<List<SpendingConditions>?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterSequenceSpendingConditions.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<List<SpendingConditions>?>(
      result.value,
      result.bytesRead + 1,
    );
  }

  static int allocationSize([List<SpendingConditions>? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterSequenceSpendingConditions.allocationSize(value) + 1;
  }

  static RustBuffer lower(List<SpendingConditions>? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length =
        FfiConverterOptionalSequenceSpendingConditions.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalSequenceSpendingConditions.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(List<SpendingConditions>? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterSequenceSpendingConditions.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalNostrWaitInfo {
  static NostrWaitInfo? lift(RustBuffer buf) {
    return FfiConverterOptionalNostrWaitInfo.read(buf.asUint8List()).value;
  }

  static LiftRetVal<NostrWaitInfo?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = NostrWaitInfo.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<NostrWaitInfo?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([NostrWaitInfo? value]) {
    if (value == null) {
      return 1;
    }
    return NostrWaitInfo.allocationSize(value) + 1;
  }

  static RustBuffer lower(NostrWaitInfo? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalNostrWaitInfo.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalNostrWaitInfo.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(NostrWaitInfo? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return NostrWaitInfo.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterSequenceSpendingConditions {
  static List<SpendingConditions> lift(RustBuffer buf) {
    return FfiConverterSequenceSpendingConditions.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<SpendingConditions>> read(Uint8List buf) {
    List<SpendingConditions> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterSpendingConditions.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<SpendingConditions> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterSpendingConditions.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<SpendingConditions> value) {
    return value
            .map((l) => FfiConverterSpendingConditions.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<SpendingConditions> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalKeys {
  static Keys? lift(RustBuffer buf) {
    return FfiConverterOptionalKeys.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Keys?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterKeys.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<Keys?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([Keys? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterKeys.allocationSize(value) + 1;
  }

  static RustBuffer lower(Keys? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalKeys.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalKeys.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(Keys? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterKeys.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalSequenceContactInfo {
  static List<ContactInfo>? lift(RustBuffer buf) {
    return FfiConverterOptionalSequenceContactInfo.read(
      buf.asUint8List(),
    ).value;
  }

  static LiftRetVal<List<ContactInfo>?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterSequenceContactInfo.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<List<ContactInfo>?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([List<ContactInfo>? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterSequenceContactInfo.allocationSize(value) + 1;
  }

  static RustBuffer lower(List<ContactInfo>? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalSequenceContactInfo.allocationSize(
      value,
    );
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalSequenceContactInfo.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(List<ContactInfo>? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterSequenceContactInfo.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterSequenceContactInfo {
  static List<ContactInfo> lift(RustBuffer buf) {
    return FfiConverterSequenceContactInfo.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<ContactInfo>> read(Uint8List buf) {
    List<ContactInfo> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterContactInfo.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<ContactInfo> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterContactInfo.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<ContactInfo> value) {
    return value
            .map((l) => FfiConverterContactInfo.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<ContactInfo> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterSequenceMintQuote {
  static List<MintQuote> lift(RustBuffer buf) {
    return FfiConverterSequenceMintQuote.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<MintQuote>> read(Uint8List buf) {
    List<MintQuote> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterMintQuote.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<MintQuote> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterMintQuote.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<MintQuote> value) {
    return value
            .map((l) => FfiConverterMintQuote.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<MintQuote> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterSequenceProtectedEndpoint {
  static List<ProtectedEndpoint> lift(RustBuffer buf) {
    return FfiConverterSequenceProtectedEndpoint.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<ProtectedEndpoint>> read(Uint8List buf) {
    List<ProtectedEndpoint> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterProtectedEndpoint.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<ProtectedEndpoint> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterProtectedEndpoint.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<ProtectedEndpoint> value) {
    return value
            .map((l) => FfiConverterProtectedEndpoint.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<ProtectedEndpoint> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalClearAuthSettings {
  static ClearAuthSettings? lift(RustBuffer buf) {
    return FfiConverterOptionalClearAuthSettings.read(buf.asUint8List()).value;
  }

  static LiftRetVal<ClearAuthSettings?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterClearAuthSettings.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<ClearAuthSettings?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([ClearAuthSettings? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterClearAuthSettings.allocationSize(value) + 1;
  }

  static RustBuffer lower(ClearAuthSettings? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalClearAuthSettings.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalClearAuthSettings.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(ClearAuthSettings? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterClearAuthSettings.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalAmount {
  static Amount? lift(RustBuffer buf) {
    return FfiConverterOptionalAmount.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Amount?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterAmount.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<Amount?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([Amount? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterAmount.allocationSize(value) + 1;
  }

  static RustBuffer lower(Amount? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalAmount.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalAmount.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(Amount? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterAmount.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterSequencePublicKey {
  static List<PublicKey> lift(RustBuffer buf) {
    return FfiConverterSequencePublicKey.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<PublicKey>> read(Uint8List buf) {
    List<PublicKey> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterPublicKey.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<PublicKey> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterPublicKey.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<PublicKey> value) {
    return value
            .map((l) => FfiConverterPublicKey.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<PublicKey> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalMeltQuote {
  static MeltQuote? lift(RustBuffer buf) {
    return FfiConverterOptionalMeltQuote.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MeltQuote?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterMeltQuote.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<MeltQuote?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([MeltQuote? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterMeltQuote.allocationSize(value) + 1;
  }

  static RustBuffer lower(MeltQuote? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalMeltQuote.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalMeltQuote.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(MeltQuote? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterMeltQuote.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalMintVersion {
  static MintVersion? lift(RustBuffer buf) {
    return FfiConverterOptionalMintVersion.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MintVersion?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterMintVersion.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<MintVersion?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([MintVersion? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterMintVersion.allocationSize(value) + 1;
  }

  static RustBuffer lower(MintVersion? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalMintVersion.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalMintVersion.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(MintVersion? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterMintVersion.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalMintQuote {
  static MintQuote? lift(RustBuffer buf) {
    return FfiConverterOptionalMintQuote.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MintQuote?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterMintQuote.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<MintQuote?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([MintQuote? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterMintQuote.allocationSize(value) + 1;
  }

  static RustBuffer lower(MintQuote? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalMintQuote.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalMintQuote.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(MintQuote? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterMintQuote.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalPaymentMethod {
  static PaymentMethod? lift(RustBuffer buf) {
    return FfiConverterOptionalPaymentMethod.read(buf.asUint8List()).value;
  }

  static LiftRetVal<PaymentMethod?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterPaymentMethod.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<PaymentMethod?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([PaymentMethod? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterPaymentMethod.allocationSize(value) + 1;
  }

  static RustBuffer lower(PaymentMethod? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalPaymentMethod.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalPaymentMethod.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(PaymentMethod? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterPaymentMethod.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterSequenceAuthProof {
  static List<AuthProof> lift(RustBuffer buf) {
    return FfiConverterSequenceAuthProof.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<AuthProof>> read(Uint8List buf) {
    List<AuthProof> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterAuthProof.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<AuthProof> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterAuthProof.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<AuthProof> value) {
    return value
            .map((l) => FfiConverterAuthProof.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<AuthProof> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterSequenceAmount {
  static List<Amount> lift(RustBuffer buf) {
    return FfiConverterSequenceAmount.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<Amount>> read(Uint8List buf) {
    List<Amount> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterAmount.read(Uint8List.view(buf.buffer, offset));
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<Amount> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterAmount.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<Amount> value) {
    return value
            .map((l) => FfiConverterAmount.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<Amount> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalBlindAuthSettings {
  static BlindAuthSettings? lift(RustBuffer buf) {
    return FfiConverterOptionalBlindAuthSettings.read(buf.asUint8List()).value;
  }

  static LiftRetVal<BlindAuthSettings?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterBlindAuthSettings.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<BlindAuthSettings?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([BlindAuthSettings? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterBlindAuthSettings.allocationSize(value) + 1;
  }

  static RustBuffer lower(BlindAuthSettings? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalBlindAuthSettings.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalBlindAuthSettings.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(BlindAuthSettings? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterBlindAuthSettings.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalMeltOptions {
  static MeltOptions? lift(RustBuffer buf) {
    return FfiConverterOptionalMeltOptions.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MeltOptions?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterMeltOptions.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<MeltOptions?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([MeltOptions? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterMeltOptions.allocationSize(value) + 1;
  }

  static RustBuffer lower(MeltOptions? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalMeltOptions.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalMeltOptions.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(MeltOptions? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterMeltOptions.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterSequenceMeltQuote {
  static List<MeltQuote> lift(RustBuffer buf) {
    return FfiConverterSequenceMeltQuote.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<MeltQuote>> read(Uint8List buf) {
    List<MeltQuote> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterMeltQuote.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<MeltQuote> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterMeltQuote.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<MeltQuote> value) {
    return value
            .map((l) => FfiConverterMeltQuote.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<MeltQuote> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterSequenceProofState {
  static List<ProofState> lift(RustBuffer buf) {
    return FfiConverterSequenceProofState.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<ProofState>> read(Uint8List buf) {
    List<ProofState> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterProofState.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<ProofState> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterProofState.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<ProofState> value) {
    return value
            .map((l) => FfiConverterProofState.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<ProofState> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterUInt32 {
  static int lift(int value) => value;
  static LiftRetVal<int> read(Uint8List buf) {
    return LiftRetVal(buf.buffer.asByteData(buf.offsetInBytes).getUint32(0), 4);
  }

  static int lower(int value) {
    if (value < 0 || value > 4294967295) {
      throw ArgumentError("Value out of range for u32: " + value.toString());
    }
    return value;
  }

  static int allocationSize([int value = 0]) {
    return 4;
  }

  static int write(int value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setUint32(0, lower(value));
    return 4;
  }
}

class FfiConverterOptionalUInt32 {
  static int? lift(RustBuffer buf) {
    return FfiConverterOptionalUInt32.read(buf.asUint8List()).value;
  }

  static LiftRetVal<int?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterUInt32.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<int?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([int? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterUInt32.allocationSize(value) + 1;
  }

  static RustBuffer lower(int? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalUInt32.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalUInt32.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(int? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterUInt32.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterString {
  static String lift(RustBuffer buf) {
    return utf8.decoder.convert(buf.asUint8List());
  }

  static RustBuffer lower(String value) {
    return toRustBuffer(Utf8Encoder().convert(value));
  }

  static LiftRetVal<String> read(Uint8List buf) {
    final end = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0) + 4;
    return LiftRetVal(utf8.decoder.convert(buf, 4, end), end);
  }

  static int allocationSize([String value = ""]) {
    return utf8.encoder.convert(value).length + 4;
  }

  static int write(String value, Uint8List buf) {
    final list = utf8.encoder.convert(value);
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, list.length);
    buf.setAll(4, list);
    return list.length + 4;
  }
}

class FfiConverterUint8List {
  static Uint8List lift(RustBuffer value) {
    return FfiConverterUint8List.read(value.asUint8List()).value;
  }

  static LiftRetVal<Uint8List> read(Uint8List buf) {
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    final bytes = Uint8List.view(buf.buffer, buf.offsetInBytes + 4, length);
    return LiftRetVal(bytes, length + 4);
  }

  static RustBuffer lower(Uint8List value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }

  static int allocationSize([Uint8List? value]) {
    if (value == null) {
      return 4;
    }
    return 4 + value.length;
  }

  static int write(Uint8List value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    buf.setRange(4, 4 + value.length, value);
    return 4 + value.length;
  }
}

class FfiConverterSequenceSecretKey {
  static List<SecretKey> lift(RustBuffer buf) {
    return FfiConverterSequenceSecretKey.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<SecretKey>> read(Uint8List buf) {
    List<SecretKey> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterSecretKey.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<SecretKey> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterSecretKey.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<SecretKey> value) {
    return value
            .map((l) => FfiConverterSecretKey.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<SecretKey> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterSequenceMintUrl {
  static List<MintUrl> lift(RustBuffer buf) {
    return FfiConverterSequenceMintUrl.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<MintUrl>> read(Uint8List buf) {
    List<MintUrl> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterMintUrl.read(Uint8List.view(buf.buffer, offset));
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<MintUrl> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterMintUrl.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<MintUrl> value) {
    return value
            .map((l) => FfiConverterMintUrl.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<MintUrl> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalSpendingConditions {
  static SpendingConditions? lift(RustBuffer buf) {
    return FfiConverterOptionalSpendingConditions.read(buf.asUint8List()).value;
  }

  static LiftRetVal<SpendingConditions?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterSpendingConditions.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<SpendingConditions?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([SpendingConditions? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterSpendingConditions.allocationSize(value) + 1;
  }

  static RustBuffer lower(SpendingConditions? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalSpendingConditions.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalSpendingConditions.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(SpendingConditions? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterSpendingConditions.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterSequenceMeltMethodSettings {
  static List<MeltMethodSettings> lift(RustBuffer buf) {
    return FfiConverterSequenceMeltMethodSettings.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<MeltMethodSettings>> read(Uint8List buf) {
    List<MeltMethodSettings> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterMeltMethodSettings.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<MeltMethodSettings> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterMeltMethodSettings.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<MeltMethodSettings> value) {
    return value
            .map((l) => FfiConverterMeltMethodSettings.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<MeltMethodSettings> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalProofDleq {
  static ProofDleq? lift(RustBuffer buf) {
    return FfiConverterOptionalProofDleq.read(buf.asUint8List()).value;
  }

  static LiftRetVal<ProofDleq?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterProofDleq.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<ProofDleq?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([ProofDleq? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterProofDleq.allocationSize(value) + 1;
  }

  static RustBuffer lower(ProofDleq? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalProofDleq.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalProofDleq.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(ProofDleq? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterProofDleq.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalMintInfo {
  static MintInfo? lift(RustBuffer buf) {
    return FfiConverterOptionalMintInfo.read(buf.asUint8List()).value;
  }

  static LiftRetVal<MintInfo?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterMintInfo.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<MintInfo?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([MintInfo? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterMintInfo.allocationSize(value) + 1;
  }

  static RustBuffer lower(MintInfo? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalMintInfo.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalMintInfo.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(MintInfo? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterMintInfo.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterSequenceBool {
  static List<bool> lift(RustBuffer buf) {
    return FfiConverterSequenceBool.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<bool>> read(Uint8List buf) {
    List<bool> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterBool.read(Uint8List.view(buf.buffer, offset));
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<bool> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterBool.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<bool> value) {
    return value
            .map((l) => FfiConverterBool.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<bool> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalUint8List {
  static Uint8List? lift(RustBuffer buf) {
    return FfiConverterOptionalUint8List.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Uint8List?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterUint8List.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<Uint8List?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([Uint8List? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterUint8List.allocationSize(value) + 1;
  }

  static RustBuffer lower(Uint8List? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalUint8List.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalUint8List.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(Uint8List? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterUint8List.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterOptionalNotificationPayload {
  static NotificationPayload? lift(RustBuffer buf) {
    return FfiConverterOptionalNotificationPayload.read(
      buf.asUint8List(),
    ).value;
  }

  static LiftRetVal<NotificationPayload?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterNotificationPayload.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<NotificationPayload?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([NotificationPayload? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterNotificationPayload.allocationSize(value) + 1;
  }

  static RustBuffer lower(NotificationPayload? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalNotificationPayload.allocationSize(
      value,
    );
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalNotificationPayload.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(NotificationPayload? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterNotificationPayload.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterSequenceSequenceString {
  static List<List<String>> lift(RustBuffer buf) {
    return FfiConverterSequenceSequenceString.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<List<String>>> read(Uint8List buf) {
    List<List<String>> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterSequenceString.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<List<String>> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterSequenceString.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<List<String>> value) {
    return value
            .map((l) => FfiConverterSequenceString.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<List<String>> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

class FfiConverterOptionalTransaction {
  static Transaction? lift(RustBuffer buf) {
    return FfiConverterOptionalTransaction.read(buf.asUint8List()).value;
  }

  static LiftRetVal<Transaction?> read(Uint8List buf) {
    if (ByteData.view(buf.buffer, buf.offsetInBytes).getInt8(0) == 0) {
      return LiftRetVal(null, 1);
    }
    final result = FfiConverterTransaction.read(
      Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
    );
    return LiftRetVal<Transaction?>(result.value, result.bytesRead + 1);
  }

  static int allocationSize([Transaction? value]) {
    if (value == null) {
      return 1;
    }
    return FfiConverterTransaction.allocationSize(value) + 1;
  }

  static RustBuffer lower(Transaction? value) {
    if (value == null) {
      return toRustBuffer(Uint8List.fromList([0]));
    }
    final length = FfiConverterOptionalTransaction.allocationSize(value);
    final Pointer<Uint8> frameData = calloc<Uint8>(length);
    final buf = frameData.asTypedList(length);
    FfiConverterOptionalTransaction.write(value, buf);
    final bytes = calloc<ForeignBytes>();
    bytes.ref.len = length;
    bytes.ref.data = frameData;
    return RustBuffer.fromBytes(bytes.ref);
  }

  static int write(Transaction? value, Uint8List buf) {
    if (value == null) {
      buf[0] = 0;
      return 1;
    }
    buf[0] = 1;
    return FfiConverterTransaction.write(
          value,
          Uint8List.view(buf.buffer, buf.offsetInBytes + 1),
        ) +
        1;
  }
}

class FfiConverterSequenceKeySetInfo {
  static List<KeySetInfo> lift(RustBuffer buf) {
    return FfiConverterSequenceKeySetInfo.read(buf.asUint8List()).value;
  }

  static LiftRetVal<List<KeySetInfo>> read(Uint8List buf) {
    List<KeySetInfo> res = [];
    final length = buf.buffer.asByteData(buf.offsetInBytes).getInt32(0);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < length; i++) {
      final ret = FfiConverterKeySetInfo.read(
        Uint8List.view(buf.buffer, offset),
      );
      offset += ret.bytesRead;
      res.add(ret.value);
    }
    return LiftRetVal(res, offset - buf.offsetInBytes);
  }

  static int write(List<KeySetInfo> value, Uint8List buf) {
    buf.buffer.asByteData(buf.offsetInBytes).setInt32(0, value.length);
    int offset = buf.offsetInBytes + 4;
    for (var i = 0; i < value.length; i++) {
      offset += FfiConverterKeySetInfo.write(
        value[i],
        Uint8List.view(buf.buffer, offset),
      );
    }
    return offset - buf.offsetInBytes;
  }

  static int allocationSize(List<KeySetInfo> value) {
    return value
            .map((l) => FfiConverterKeySetInfo.allocationSize(l))
            .fold(0, (a, b) => a + b) +
        4;
  }

  static RustBuffer lower(List<KeySetInfo> value) {
    final buf = Uint8List(allocationSize(value));
    write(value, buf);
    return toRustBuffer(buf);
  }
}

const int UNIFFI_RUST_FUTURE_POLL_READY = 0;
const int UNIFFI_RUST_FUTURE_POLL_MAYBE_READY = 1;
typedef UniffiRustFutureContinuationCallback = Void Function(Uint64, Int8);
final _uniffiRustFutureContinuationHandles = UniffiHandleMap<Completer<int>>();
Future<T> uniffiRustCallAsync<T, F>(
  Pointer<Void> Function() rustFutureFunc,
  void Function(
    Pointer<Void>,
    Pointer<NativeFunction<UniffiRustFutureContinuationCallback>>,
    Pointer<Void>,
  )
  pollFunc,
  F Function(Pointer<Void>, Pointer<RustCallStatus>) completeFunc,
  void Function(Pointer<Void>) freeFunc,
  T Function(F) liftFunc, [
  UniffiRustCallStatusErrorHandler? errorHandler,
]) async {
  final rustFuture = rustFutureFunc();
  final completer = Completer<int>();
  final handle = _uniffiRustFutureContinuationHandles.insert(completer);
  final callbackData = Pointer<Void>.fromAddress(handle);
  late final NativeCallable<UniffiRustFutureContinuationCallback> callback;
  void repoll() {
    pollFunc(rustFuture, callback.nativeFunction, callbackData);
  }

  void onResponse(int data, int pollResult) {
    if (pollResult == UNIFFI_RUST_FUTURE_POLL_READY) {
      final readyCompleter = _uniffiRustFutureContinuationHandles.maybeRemove(
        data,
      );
      if (readyCompleter != null && !readyCompleter.isCompleted) {
        readyCompleter.complete(pollResult);
      }
    } else if (pollResult == UNIFFI_RUST_FUTURE_POLL_MAYBE_READY) {
      repoll();
    } else {
      final errorCompleter = _uniffiRustFutureContinuationHandles.maybeRemove(
        data,
      );
      if (errorCompleter != null && !errorCompleter.isCompleted) {
        errorCompleter.completeError(
          UniffiInternalError.panicked(
            "Unexpected poll result from Rust future: \$pollResult",
          ),
        );
      }
    }
  }

  callback = NativeCallable<UniffiRustFutureContinuationCallback>.listener(
    onResponse,
  );
  try {
    repoll();
    await completer.future;
    final status = calloc<RustCallStatus>();
    try {
      final result = completeFunc(rustFuture, status);
      checkCallStatus(errorHandler ?? NullRustCallStatusErrorHandler(), status);
      return liftFunc(result);
    } finally {
      calloc.free(status);
    }
  } finally {
    callback.close();
    _uniffiRustFutureContinuationHandles.maybeRemove(handle);
    freeFunc(rustFuture);
  }
}

typedef UniffiForeignFutureFree = Void Function(Uint64);
typedef UniffiForeignFutureFreeDart = void Function(int);

class _UniffiForeignFutureState {
  bool cancelled = false;
}

final _uniffiForeignFutureHandleMap =
    UniffiHandleMap<_UniffiForeignFutureState>();
void _uniffiForeignFutureFree(int handle) {
  final state = _uniffiForeignFutureHandleMap.maybeRemove(handle);
  if (state != null) {
    state.cancelled = true;
  }
}

final Pointer<NativeFunction<UniffiForeignFutureFree>>
_uniffiForeignFutureFreePointer = Pointer.fromFunction<UniffiForeignFutureFree>(
  _uniffiForeignFutureFree,
);

final class UniffiForeignFuture extends Struct {
  @Uint64()
  external int handle;
  external Pointer<NativeFunction<UniffiForeignFutureFree>> free;
}

class UniffiHandleMap<T> {
  final Map<int, T> _map = {};
  int _counter = 1;
  int insert(T obj) {
    final handle = _counter;
    _counter += 2;
    _map[handle] = obj;
    return handle;
  }

  T get(int handle) {
    final obj = _map[handle];
    if (obj == null) {
      throw UniffiInternalError(
        UniffiInternalError.unexpectedStaleHandle,
        "Handle not found",
      );
    }
    return obj;
  }

  void remove(int handle) {
    if (maybeRemove(handle) == null) {
      throw UniffiInternalError(
        UniffiInternalError.unexpectedStaleHandle,
        "Handle not found",
      );
    }
  }

  T? maybeRemove(int handle) {
    return _map.remove(handle);
  }
}

const _uniffiAssetId = "package:cdk/uniffi:cdk";
WalletDatabase createWalletDb({required WalletDbBackend backend}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_create_wallet_db(
      FfiConverterWalletDbBackend.lower(backend),
      status,
    ),
    FfiConverterCallbackInterfaceWalletDatabase.lift,
    ffiExceptionErrorHandler,
  );
}

AuthProof decodeAuthProof({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_auth_proof(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterAuthProof.lift,
    ffiExceptionErrorHandler,
  );
}

Conditions decodeConditions({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_conditions(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterConditions.lift,
    ffiExceptionErrorHandler,
  );
}

ContactInfo decodeContactInfo({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_contact_info(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterContactInfo.lift,
    ffiExceptionErrorHandler,
  );
}

CreateRequestParams decodeCreateRequestParams({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_create_request_params(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterCreateRequestParams.lift,
    ffiExceptionErrorHandler,
  );
}

DecodedInvoice decodeInvoice({required String invoiceStr}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_invoice(
      FfiConverterString.lower(invoiceStr),
      status,
    ),
    FfiConverterDecodedInvoice.lift,
    ffiExceptionErrorHandler,
  );
}

KeySet decodeKeySet({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_key_set(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterKeySet.lift,
    ffiExceptionErrorHandler,
  );
}

KeySetInfo decodeKeySetInfo({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_key_set_info(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterKeySetInfo.lift,
    ffiExceptionErrorHandler,
  );
}

Keys decodeKeys({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_keys(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterKeys.lift,
    ffiExceptionErrorHandler,
  );
}

MeltQuote decodeMeltQuote({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_melt_quote(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterMeltQuote.lift,
    ffiExceptionErrorHandler,
  );
}

MintInfo decodeMintInfo({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_mint_info(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterMintInfo.lift,
    ffiExceptionErrorHandler,
  );
}

MintQuote decodeMintQuote({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_mint_quote(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterMintQuote.lift,
    ffiExceptionErrorHandler,
  );
}

MintVersion decodeMintVersion({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_mint_version(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterMintVersion.lift,
    ffiExceptionErrorHandler,
  );
}

Nuts decodeNuts({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_nuts(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterNuts.lift,
    ffiExceptionErrorHandler,
  );
}

PaymentRequest decodePaymentRequest({required String encoded}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_payment_request(
      FfiConverterString.lower(encoded),
      status,
    ),
    PaymentRequest.lift,
    ffiExceptionErrorHandler,
  );
}

ProofInfo decodeProofInfo({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_proof_info(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterProofInfo.lift,
    ffiExceptionErrorHandler,
  );
}

ProofStateUpdate decodeProofStateUpdate({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_proof_state_update(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterProofStateUpdate.lift,
    ffiExceptionErrorHandler,
  );
}

ReceiveOptions decodeReceiveOptions({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_receive_options(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterReceiveOptions.lift,
    ffiExceptionErrorHandler,
  );
}

SendMemo decodeSendMemo({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_send_memo(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterSendMemo.lift,
    ffiExceptionErrorHandler,
  );
}

SendOptions decodeSendOptions({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_send_options(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterSendOptions.lift,
    ffiExceptionErrorHandler,
  );
}

SubscribeParams decodeSubscribeParams({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_subscribe_params(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterSubscribeParams.lift,
    ffiExceptionErrorHandler,
  );
}

Transaction decodeTransaction({required String json}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_decode_transaction(
      FfiConverterString.lower(json),
      status,
    ),
    FfiConverterTransaction.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeAuthProof({required AuthProof proof}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_auth_proof(
      FfiConverterAuthProof.lower(proof),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeConditions({required Conditions conditions}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_conditions(
      FfiConverterConditions.lower(conditions),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeContactInfo({required ContactInfo info}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_contact_info(
      FfiConverterContactInfo.lower(info),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeCreateRequestParams({required CreateRequestParams params}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_create_request_params(
      FfiConverterCreateRequestParams.lower(params),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeKeySet({required KeySet keyset}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_key_set(
      FfiConverterKeySet.lower(keyset),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeKeySetInfo({required KeySetInfo info}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_key_set_info(
      FfiConverterKeySetInfo.lower(info),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeKeys({required Keys keys}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_keys(
      FfiConverterKeys.lower(keys),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeMeltQuote({required MeltQuote quote}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_melt_quote(
      FfiConverterMeltQuote.lower(quote),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeMintInfo({required MintInfo info}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_mint_info(
      FfiConverterMintInfo.lower(info),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeMintQuote({required MintQuote quote}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_mint_quote(
      FfiConverterMintQuote.lower(quote),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeMintVersion({required MintVersion version}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_mint_version(
      FfiConverterMintVersion.lower(version),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeNuts({required Nuts nuts}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_nuts(
      FfiConverterNuts.lower(nuts),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeProofInfo({required ProofInfo info}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_proof_info(
      FfiConverterProofInfo.lower(info),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeProofStateUpdate({required ProofStateUpdate update}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_proof_state_update(
      FfiConverterProofStateUpdate.lower(update),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeReceiveOptions({required ReceiveOptions options}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_receive_options(
      FfiConverterReceiveOptions.lower(options),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeSendMemo({required SendMemo memo}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_send_memo(
      FfiConverterSendMemo.lower(memo),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeSendOptions({required SendOptions options}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_send_options(
      FfiConverterSendOptions.lower(options),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeSubscribeParams({required SubscribeParams params}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_subscribe_params(
      FfiConverterSubscribeParams.lower(params),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String encodeTransaction({required Transaction transaction}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_encode_transaction(
      FfiConverterTransaction.lower(transaction),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String generateMnemonic() {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_generate_mnemonic(status),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

void initDefaultLogging() {
  return rustCall((status) {
    uniffi_cdk_ffi_fn_func_init_default_logging(status);
  }, null);
}

void initLogging({required String level}) {
  return rustCall((status) {
    uniffi_cdk_ffi_fn_func_init_logging(
      FfiConverterString.lower(level),
      status,
    );
  }, null);
}

Amount mintQuoteAmountMintable({required MintQuote quote}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_mint_quote_amount_mintable(
      FfiConverterMintQuote.lower(quote),
      status,
    ),
    FfiConverterAmount.lift,
    ffiExceptionErrorHandler,
  );
}

bool mintQuoteIsExpired({required MintQuote quote, required int currentTime}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_mint_quote_is_expired(
      FfiConverterMintQuote.lower(quote),
      FfiConverterUInt64.lower(currentTime),
      status,
    ),
    FfiConverterBool.lift,
    ffiExceptionErrorHandler,
  );
}

Amount mintQuoteTotalAmount({required MintQuote quote}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_mint_quote_total_amount(
      FfiConverterMintQuote.lower(quote),
      status,
    ),
    FfiConverterAmount.lift,
    ffiExceptionErrorHandler,
  );
}

Uint8List mnemonicToEntropy({required String mnemonic}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_mnemonic_to_entropy(
      FfiConverterString.lower(mnemonic),
      status,
    ),
    FfiConverterUint8List.lift,
    ffiExceptionErrorHandler,
  );
}

String npubcashDeriveSecretKeyFromSeed({required Uint8List seed}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_npubcash_derive_secret_key_from_seed(
      FfiConverterUint8List.lower(seed),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

String npubcashGetPubkey({required String nostrSecretKey}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_npubcash_get_pubkey(
      FfiConverterString.lower(nostrSecretKey),
      status,
    ),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

MintQuote npubcashQuoteToMintQuote({required NpubCashQuote quote}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_npubcash_quote_to_mint_quote(
      FfiConverterNpubCashQuote.lower(quote),
      status,
    ),
    FfiConverterMintQuote.lift,
    null,
  );
}

bool proofHasDleq({required Proof proof}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_proof_has_dleq(
      FfiConverterProof.lower(proof),
      status,
    ),
    FfiConverterBool.lift,
    null,
  );
}

bool proofIsActive({
  required Proof proof,
  required List<String> activeKeysetIds,
}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_proof_is_active(
      FfiConverterProof.lower(proof),
      FfiConverterSequenceString.lower(activeKeysetIds),
      status,
    ),
    FfiConverterBool.lift,
    null,
  );
}

Proof proofSignP2pk({required Proof proof, required String secretKeyHex}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_proof_sign_p2pk(
      FfiConverterProof.lower(proof),
      FfiConverterString.lower(secretKeyHex),
      status,
    ),
    FfiConverterProof.lift,
    ffiExceptionErrorHandler,
  );
}

void proofVerifyDleq({required Proof proof, required PublicKey mintPubkey}) {
  return rustCall((status) {
    uniffi_cdk_ffi_fn_func_proof_verify_dleq(
      FfiConverterProof.lower(proof),
      FfiConverterPublicKey.lower(mintPubkey),
      status,
    );
  }, ffiExceptionErrorHandler);
}

void proofVerifyHtlc({required Proof proof}) {
  return rustCall((status) {
    uniffi_cdk_ffi_fn_func_proof_verify_htlc(
      FfiConverterProof.lower(proof),
      status,
    );
  }, ffiExceptionErrorHandler);
}

String proofY({required Proof proof}) {
  return rustCallWithLifter(
    (status) =>
        uniffi_cdk_ffi_fn_func_proof_y(FfiConverterProof.lower(proof), status),
    FfiConverterString.lift,
    ffiExceptionErrorHandler,
  );
}

Amount proofsTotalAmount({required List<Proof> proofs}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_proofs_total_amount(
      FfiConverterSequenceProof.lower(proofs),
      status,
    ),
    FfiConverterAmount.lift,
    ffiExceptionErrorHandler,
  );
}

bool transactionMatchesConditions({
  required Transaction transaction,
  required MintUrl? mintUrl,
  required TransactionDirection? direction,
  required CurrencyUnit? unit,
}) {
  return rustCallWithLifter(
    (status) => uniffi_cdk_ffi_fn_func_transaction_matches_conditions(
      FfiConverterTransaction.lower(transaction),
      FfiConverterOptionalMintUrl.lower(mintUrl),
      FfiConverterOptionalTransactionDirection.lower(direction),
      FfiConverterOptionalCurrencyUnit.lower(unit),
      status,
    ),
    FfiConverterBool.lift,
    ffiExceptionErrorHandler,
  );
}

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_clone_activesubscription(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_free_activesubscription(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_activesubscription_id(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_activesubscription_recv(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_activesubscription_try_recv(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_clone_nostrwaitinfo(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_free_nostrwaitinfo(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_nostrwaitinfo_pubkey(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_nostrwaitinfo_relays(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_clone_npubcashclient(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_free_npubcashclient(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Pointer<Void> Function(RustBuffer, RustBuffer, Pointer<RustCallStatus>)
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_constructor_npubcashclient_new(
  RustBuffer base_url,
  RustBuffer nostr_secret_key,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_npubcashclient_get_quotes(
  Pointer<Void> ptr,
  RustBuffer since,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_npubcashclient_set_mint_url(
  Pointer<Void> ptr,
  RustBuffer mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_clone_paymentrequest(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_free_paymentrequest(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_constructor_paymentrequest_from_string(
  RustBuffer encoded,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_paymentrequest_amount(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_paymentrequest_description(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_paymentrequest_mints(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_paymentrequest_payment_id(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_paymentrequest_single_use(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_paymentrequest_to_string_encoded(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_paymentrequest_transports(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_paymentrequest_unit(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_clone_paymentrequestpayload(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_free_paymentrequestpayload(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_constructor_paymentrequestpayload_from_string(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_paymentrequestpayload_id(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_paymentrequestpayload_memo(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_paymentrequestpayload_mint(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_paymentrequestpayload_proofs(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_paymentrequestpayload_unit(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_clone_preparedmelt(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_free_preparedmelt(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_preparedmelt_amount(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_preparedmelt_cancel(
  Pointer<Void> ptr,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer
uniffi_cdk_ffi_fn_method_preparedmelt_change_amount_without_swap(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_preparedmelt_confirm(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_preparedmelt_confirm_with_options(
  Pointer<Void> ptr,
  RustBuffer options,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_preparedmelt_fee_reserve(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer
uniffi_cdk_ffi_fn_method_preparedmelt_fee_savings_without_swap(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_preparedmelt_input_fee(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer
uniffi_cdk_ffi_fn_method_preparedmelt_input_fee_without_swap(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_preparedmelt_operation_id(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_preparedmelt_proofs(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_preparedmelt_quote_id(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Int8 Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external int uniffi_cdk_ffi_fn_method_preparedmelt_requires_swap(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_preparedmelt_swap_fee(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_preparedmelt_total_fee(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_preparedmelt_total_fee_with_swap(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_clone_preparedsend(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_free_preparedsend(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_preparedsend_amount(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_preparedsend_cancel(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_preparedsend_confirm(
  Pointer<Void> ptr,
  RustBuffer memo,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_preparedsend_fee(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_preparedsend_operation_id(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_preparedsend_proofs(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_clone_token(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_free_token(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_constructor_token_decode(
  RustBuffer encoded_token,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_constructor_token_from_raw_bytes(
  RustBuffer bytes,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_constructor_token_from_string(
  RustBuffer encoded_token,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_token_encode(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_token_htlc_hashes(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_token_locktimes(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_token_memo(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_token_mint_url(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_token_p2pk_pubkeys(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_token_p2pk_refund_pubkeys(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  RustBuffer Function(Pointer<Void>, RustBuffer, Pointer<RustCallStatus>)
>(assetId: _uniffiAssetId)
external RustBuffer uniffi_cdk_ffi_fn_method_token_proofs(
  Pointer<Void> ptr,
  RustBuffer mint_keysets,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_token_proofs_simple(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_token_spending_conditions(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_token_to_raw_bytes(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_token_unit(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_token_value(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_clone_wallet(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_free_wallet(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Pointer<Void> Function(
    RustBuffer,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    Pointer<RustCallStatus>,
  )
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_constructor_wallet_new(
  RustBuffer mint_url,
  RustBuffer unit,
  RustBuffer mnemonic,
  RustBuffer store,
  RustBuffer config,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>, Uint32, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_calculate_fee(
  Pointer<Void> ptr,
  int proof_count,
  RustBuffer keyset_id,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_check_all_pending_proofs(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_check_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_check_proofs_spent(
  Pointer<Void> ptr,
  RustBuffer proofs,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_check_send_status(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_fetch_mint_info(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_fetch_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
  RustBuffer payment_method,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_get_active_keyset(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_get_keyset_fees_by_id(
  Pointer<Void> ptr,
  RustBuffer keyset_id,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_get_pending_sends(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_get_proofs_by_states(
  Pointer<Void> ptr,
  RustBuffer states,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_wallet_get_proofs_for_transaction(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_get_transaction(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_get_unspent_auth_proofs(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_list_transactions(
  Pointer<Void> ptr,
  RustBuffer direction,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_load_mint_info(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_melt_bip353_quote(
  Pointer<Void> ptr,
  RustBuffer bip353_address,
  RustBuffer amount_msat,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_melt_human_readable(
  Pointer<Void> ptr,
  RustBuffer address,
  RustBuffer amount_msat,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_wallet_melt_lightning_address_quote(
  Pointer<Void> ptr,
  RustBuffer lightning_address,
  RustBuffer amount_msat,
);

@Native<
  Pointer<Void> Function(
    Pointer<Void>,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    RustBuffer,
  )
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_melt_quote(
  Pointer<Void> ptr,
  RustBuffer method,
  RustBuffer request,
  RustBuffer options,
  RustBuffer extra,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_mint(
  Pointer<Void> ptr,
  RustBuffer quote_id,
  RustBuffer amount_split_target,
  RustBuffer spending_conditions,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_mint_blind_auth(
  Pointer<Void> ptr,
  RustBuffer amount,
);

@Native<
  Pointer<Void> Function(
    Pointer<Void>,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    RustBuffer,
  )
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_mint_quote(
  Pointer<Void> ptr,
  RustBuffer payment_method,
  RustBuffer amount,
  RustBuffer description,
  RustBuffer extra,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_mint_unified(
  Pointer<Void> ptr,
  RustBuffer quote_id,
  RustBuffer amount_split_target,
  RustBuffer spending_conditions,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_wallet_mint_url(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_pay_request(
  Pointer<Void> ptr,
  Pointer<Void> payment_request,
  RustBuffer custom_amount,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_prepare_melt(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_prepare_melt_proofs(
  Pointer<Void> ptr,
  RustBuffer quote_id,
  RustBuffer proofs,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_prepare_send(
  Pointer<Void> ptr,
  RustBuffer amount,
  RustBuffer options,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_receive(
  Pointer<Void> ptr,
  Pointer<Void> token,
  RustBuffer options,
);

@Native<
  Pointer<Void> Function(
    Pointer<Void>,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    RustBuffer,
  )
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_receive_proofs(
  Pointer<Void> ptr,
  RustBuffer proofs,
  RustBuffer options,
  RustBuffer memo,
  RustBuffer token,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_refresh_access_token(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_refresh_keysets(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_restore(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_revert_transaction(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_revoke_send(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_set_cat(
  Pointer<Void> ptr,
  RustBuffer cat,
);

@Native<Void Function(Pointer<Void>, RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_method_wallet_set_metadata_cache_ttl(
  Pointer<Void> ptr,
  RustBuffer ttl_secs,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_set_refresh_token(
  Pointer<Void> ptr,
  RustBuffer refresh_token,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_subscribe(
  Pointer<Void> ptr,
  RustBuffer params,
);

@Native<
  Pointer<Void> Function(
    Pointer<Void>,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    Int8,
  )
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_swap(
  Pointer<Void> ptr,
  RustBuffer amount,
  RustBuffer amount_split_target,
  RustBuffer input_proofs,
  RustBuffer spending_conditions,
  int include_fees,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_total_balance(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_total_pending_balance(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_total_reserved_balance(
  Pointer<Void> ptr,
);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_method_wallet_unit(
  Pointer<Void> ptr,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<Void>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_wallet_verify_token_dleq(
  Pointer<Void> ptr,
  Pointer<Void> token,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_clone_walletdatabase(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_free_walletdatabase(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<UniffiVTableCallbackInterfaceWalletDatabase>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_init_callback_vtable_walletdatabase(
  Pointer<UniffiVTableCallbackInterfaceWalletDatabase> vtable,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_mint(
  Pointer<Void> ptr,
  RustBuffer mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_mints(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_mint_keysets(
  Pointer<Void> ptr,
  RustBuffer mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_keyset_by_id(
  Pointer<Void> ptr,
  RustBuffer keyset_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_mint_quotes(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletdatabase_get_unissued_mint_quotes(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_melt_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_melt_quotes(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_keys(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<
  Pointer<Void> Function(
    Pointer<Void>,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    RustBuffer,
  )
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_proofs(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer unit,
  RustBuffer state,
  RustBuffer spending_conditions,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_proofs_by_ys(
  Pointer<Void> ptr,
  RustBuffer ys,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_balance(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer unit,
  RustBuffer state,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_transaction(
  Pointer<Void> ptr,
  RustBuffer transaction_id,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletdatabase_list_transactions(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer direction,
  RustBuffer unit,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_kv_read(
  Pointer<Void> ptr,
  RustBuffer primary_namespace,
  RustBuffer secondary_namespace,
  RustBuffer key,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_kv_list(
  Pointer<Void> ptr,
  RustBuffer primary_namespace,
  RustBuffer secondary_namespace,
);

@Native<
  Pointer<Void> Function(
    Pointer<Void>,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    RustBuffer,
  )
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_kv_write(
  Pointer<Void> ptr,
  RustBuffer primary_namespace,
  RustBuffer secondary_namespace,
  RustBuffer key,
  RustBuffer value,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_kv_remove(
  Pointer<Void> ptr,
  RustBuffer primary_namespace,
  RustBuffer secondary_namespace,
  RustBuffer key,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_update_proofs(
  Pointer<Void> ptr,
  RustBuffer added,
  RustBuffer removed_ys,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletdatabase_update_proofs_state(
  Pointer<Void> ptr,
  RustBuffer ys,
  RustBuffer state,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_add_transaction(
  Pointer<Void> ptr,
  RustBuffer transaction,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletdatabase_remove_transaction(
  Pointer<Void> ptr,
  RustBuffer transaction_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_update_mint_url(
  Pointer<Void> ptr,
  RustBuffer old_mint_url,
  RustBuffer new_mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, Uint32)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletdatabase_increment_keyset_counter(
  Pointer<Void> ptr,
  RustBuffer keyset_id,
  int count,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_add_mint(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer mint_info,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_remove_mint(
  Pointer<Void> ptr,
  RustBuffer mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_add_mint_keysets(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer keysets,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_add_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletdatabase_remove_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_add_melt_quote(
  Pointer<Void> ptr,
  RustBuffer quote,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletdatabase_remove_melt_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_add_keys(
  Pointer<Void> ptr,
  RustBuffer keyset,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_remove_keys(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_add_saga(
  Pointer<Void> ptr,
  RustBuffer saga_json,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_get_saga(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_update_saga(
  Pointer<Void> ptr,
  RustBuffer saga_json,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_delete_saga(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletdatabase_get_incomplete_sagas(Pointer<Void> ptr);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_reserve_proofs(
  Pointer<Void> ptr,
  RustBuffer ys,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletdatabase_release_proofs(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletdatabase_get_reserved_proofs(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletdatabase_reserve_melt_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletdatabase_release_melt_quote(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletdatabase_reserve_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletdatabase_release_mint_quote(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_clone_walletpostgresdatabase(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_free_walletpostgresdatabase(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_constructor_walletpostgresdatabase_new(
  RustBuffer url,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_keys(
  Pointer<Void> ptr,
  RustBuffer keyset,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_melt_quote(
  Pointer<Void> ptr,
  RustBuffer quote,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_mint(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer mint_info,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_mint_keysets(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer keysets,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_saga(
  Pointer<Void> ptr,
  RustBuffer saga_json,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_add_transaction(
  Pointer<Void> ptr,
  RustBuffer transaction,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_delete_saga(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_balance(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer unit,
  RustBuffer state,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_incomplete_sagas(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_keys(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_keyset_by_id(
  Pointer<Void> ptr,
  RustBuffer keyset_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_melt_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_melt_quotes(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_mint(
  Pointer<Void> ptr,
  RustBuffer mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_mint_keysets(
  Pointer<Void> ptr,
  RustBuffer mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_mint_quotes(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_mints(Pointer<Void> ptr);

@Native<
  Pointer<Void> Function(
    Pointer<Void>,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    RustBuffer,
  )
>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_proofs(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer unit,
  RustBuffer state,
  RustBuffer spending_conditions,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_proofs_by_ys(
  Pointer<Void> ptr,
  RustBuffer ys,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_reserved_proofs(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_saga(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_transaction(
  Pointer<Void> ptr,
  RustBuffer transaction_id,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_get_unissued_mint_quotes(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, Uint32)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_increment_keyset_counter(
  Pointer<Void> ptr,
  RustBuffer keyset_id,
  int count,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletpostgresdatabase_kv_list(
  Pointer<Void> ptr,
  RustBuffer primary_namespace,
  RustBuffer secondary_namespace,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletpostgresdatabase_kv_read(
  Pointer<Void> ptr,
  RustBuffer primary_namespace,
  RustBuffer secondary_namespace,
  RustBuffer key,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_kv_remove(
  Pointer<Void> ptr,
  RustBuffer primary_namespace,
  RustBuffer secondary_namespace,
  RustBuffer key,
);

@Native<
  Pointer<Void> Function(
    Pointer<Void>,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    RustBuffer,
  )
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletpostgresdatabase_kv_write(
  Pointer<Void> ptr,
  RustBuffer primary_namespace,
  RustBuffer secondary_namespace,
  RustBuffer key,
  RustBuffer value,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_list_transactions(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer direction,
  RustBuffer unit,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_release_melt_quote(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_release_mint_quote(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_release_proofs(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_remove_keys(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_remove_melt_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_remove_mint(
  Pointer<Void> ptr,
  RustBuffer mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_remove_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_remove_transaction(
  Pointer<Void> ptr,
  RustBuffer transaction_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_reserve_melt_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_reserve_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_reserve_proofs(
  Pointer<Void> ptr,
  RustBuffer ys,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_update_mint_url(
  Pointer<Void> ptr,
  RustBuffer old_mint_url,
  RustBuffer new_mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_update_proofs(
  Pointer<Void> ptr,
  RustBuffer added,
  RustBuffer removed_ys,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_update_proofs_state(
  Pointer<Void> ptr,
  RustBuffer ys,
  RustBuffer state,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletpostgresdatabase_update_saga(
  Pointer<Void> ptr,
  RustBuffer saga_json,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_clone_walletrepository(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_free_walletrepository(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Pointer<Void> Function(RustBuffer, RustBuffer, Pointer<RustCallStatus>)
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_constructor_walletrepository_new(
  RustBuffer mnemonic,
  RustBuffer store,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Pointer<Void> Function(
    RustBuffer,
    RustBuffer,
    RustBuffer,
    Pointer<RustCallStatus>,
  )
>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_constructor_walletrepository_new_with_proxy(
  RustBuffer mnemonic,
  RustBuffer store,
  RustBuffer proxy_url,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletrepository_create_wallet(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer unit,
  RustBuffer target_proof_count,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletrepository_get_balances(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletrepository_get_wallet(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer unit,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletrepository_get_wallets(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletrepository_has_mint(
  Pointer<Void> ptr,
  RustBuffer mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletrepository_remove_wallet(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer currency_unit,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletrepository_set_metadata_cache_ttl_for_all_mints(
  Pointer<Void> ptr,
  RustBuffer ttl_secs,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletrepository_set_metadata_cache_ttl_for_mint(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer ttl_secs,
);

@Native<Pointer<Void> Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_clone_walletsqlitedatabase(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_free_walletsqlitedatabase(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_constructor_walletsqlitedatabase_new(
  RustBuffer file_path,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_constructor_walletsqlitedatabase_new_in_memory(
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_keys(
  Pointer<Void> ptr,
  RustBuffer keyset,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_melt_quote(
  Pointer<Void> ptr,
  RustBuffer quote,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_mint(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer mint_info,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_mint_keysets(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer keysets,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_saga(
  Pointer<Void> ptr,
  RustBuffer saga_json,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_add_transaction(
  Pointer<Void> ptr,
  RustBuffer transaction,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_delete_saga(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_balance(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer unit,
  RustBuffer state,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_incomplete_sagas(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_keys(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_keyset_by_id(
  Pointer<Void> ptr,
  RustBuffer keyset_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_melt_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_melt_quotes(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_mint(
  Pointer<Void> ptr,
  RustBuffer mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_mint_keysets(
  Pointer<Void> ptr,
  RustBuffer mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_mint_quotes(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_mints(
  Pointer<Void> ptr,
);

@Native<
  Pointer<Void> Function(
    Pointer<Void>,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    RustBuffer,
  )
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_proofs(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer unit,
  RustBuffer state,
  RustBuffer spending_conditions,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_proofs_by_ys(
  Pointer<Void> ptr,
  RustBuffer ys,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_reserved_proofs(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_saga(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_transaction(
  Pointer<Void> ptr,
  RustBuffer transaction_id,
);

@Native<Pointer<Void> Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_get_unissued_mint_quotes(
  Pointer<Void> ptr,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, Uint32)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_increment_keyset_counter(
  Pointer<Void> ptr,
  RustBuffer keyset_id,
  int count,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletsqlitedatabase_kv_list(
  Pointer<Void> ptr,
  RustBuffer primary_namespace,
  RustBuffer secondary_namespace,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletsqlitedatabase_kv_read(
  Pointer<Void> ptr,
  RustBuffer primary_namespace,
  RustBuffer secondary_namespace,
  RustBuffer key,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletsqlitedatabase_kv_remove(
  Pointer<Void> ptr,
  RustBuffer primary_namespace,
  RustBuffer secondary_namespace,
  RustBuffer key,
);

@Native<
  Pointer<Void> Function(
    Pointer<Void>,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    RustBuffer,
  )
>(assetId: _uniffiAssetId)
external Pointer<Void> uniffi_cdk_ffi_fn_method_walletsqlitedatabase_kv_write(
  Pointer<Void> ptr,
  RustBuffer primary_namespace,
  RustBuffer secondary_namespace,
  RustBuffer key,
  RustBuffer value,
);

@Native<
  Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer, RustBuffer)
>(assetId: _uniffiAssetId)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_list_transactions(
  Pointer<Void> ptr,
  RustBuffer mint_url,
  RustBuffer direction,
  RustBuffer unit,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_release_melt_quote(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_release_mint_quote(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_release_proofs(
  Pointer<Void> ptr,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_remove_keys(
  Pointer<Void> ptr,
  RustBuffer id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_remove_melt_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_remove_mint(
  Pointer<Void> ptr,
  RustBuffer mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_remove_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_remove_transaction(
  Pointer<Void> ptr,
  RustBuffer transaction_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_reserve_melt_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_reserve_mint_quote(
  Pointer<Void> ptr,
  RustBuffer quote_id,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_reserve_proofs(
  Pointer<Void> ptr,
  RustBuffer ys,
  RustBuffer operation_id,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_update_mint_url(
  Pointer<Void> ptr,
  RustBuffer old_mint_url,
  RustBuffer new_mint_url,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_update_proofs(
  Pointer<Void> ptr,
  RustBuffer added,
  RustBuffer removed_ys,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_update_proofs_state(
  Pointer<Void> ptr,
  RustBuffer ys,
  RustBuffer state,
);

@Native<Pointer<Void> Function(Pointer<Void>, RustBuffer)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void>
uniffi_cdk_ffi_fn_method_walletsqlitedatabase_update_saga(
  Pointer<Void> ptr,
  RustBuffer saga_json,
);

@Native<Pointer<Void> Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_func_create_wallet_db(
  RustBuffer backend,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_auth_proof(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_conditions(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_contact_info(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_create_request_params(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_invoice(
  RustBuffer invoice_str,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_key_set(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_key_set_info(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_keys(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_melt_quote(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_mint_info(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_mint_quote(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_mint_version(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_nuts(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Pointer<Void> Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external Pointer<Void> uniffi_cdk_ffi_fn_func_decode_payment_request(
  RustBuffer encoded,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_proof_info(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_proof_state_update(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_receive_options(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_send_memo(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_send_options(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_subscribe_params(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_decode_transaction(
  RustBuffer json,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_auth_proof(
  RustBuffer proof,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_conditions(
  RustBuffer conditions,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_contact_info(
  RustBuffer info,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_create_request_params(
  RustBuffer params,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_key_set(
  RustBuffer keyset,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_key_set_info(
  RustBuffer info,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_keys(
  RustBuffer keys,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_melt_quote(
  RustBuffer quote,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_mint_info(
  RustBuffer info,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_mint_quote(
  RustBuffer quote,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_mint_version(
  RustBuffer version,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_nuts(
  RustBuffer nuts,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_proof_info(
  RustBuffer info,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_proof_state_update(
  RustBuffer update,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_receive_options(
  RustBuffer options,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_send_memo(
  RustBuffer memo,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_send_options(
  RustBuffer options,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_subscribe_params(
  RustBuffer params,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_encode_transaction(
  RustBuffer transaction,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Pointer<RustCallStatus>)>(assetId: _uniffiAssetId)
external RustBuffer uniffi_cdk_ffi_fn_func_generate_mnemonic(
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(Pointer<RustCallStatus>)>(assetId: _uniffiAssetId)
external void uniffi_cdk_ffi_fn_func_init_default_logging(
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_func_init_logging(
  RustBuffer level,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_mint_quote_amount_mintable(
  RustBuffer quote,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Int8 Function(RustBuffer, Uint64, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external int uniffi_cdk_ffi_fn_func_mint_quote_is_expired(
  RustBuffer quote,
  int current_time,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_mint_quote_total_amount(
  RustBuffer quote,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_mnemonic_to_entropy(
  RustBuffer mnemonic,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_npubcash_derive_secret_key_from_seed(
  RustBuffer seed,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_npubcash_get_pubkey(
  RustBuffer nostr_secret_key,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_npubcash_quote_to_mint_quote(
  RustBuffer quote,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Int8 Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external int uniffi_cdk_ffi_fn_func_proof_has_dleq(
  RustBuffer proof,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Int8 Function(RustBuffer, RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external int uniffi_cdk_ffi_fn_func_proof_is_active(
  RustBuffer proof,
  RustBuffer active_keyset_ids,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_proof_sign_p2pk(
  RustBuffer proof,
  RustBuffer secret_key_hex,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(RustBuffer, RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_func_proof_verify_dleq(
  RustBuffer proof,
  RustBuffer mint_pubkey,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void uniffi_cdk_ffi_fn_func_proof_verify_htlc(
  RustBuffer proof,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_proof_y(
  RustBuffer proof,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer uniffi_cdk_ffi_fn_func_proofs_total_amount(
  RustBuffer proofs,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Int8 Function(
    RustBuffer,
    RustBuffer,
    RustBuffer,
    RustBuffer,
    Pointer<RustCallStatus>,
  )
>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_fn_func_transaction_matches_conditions(
  RustBuffer transaction,
  RustBuffer mint_url,
  RustBuffer direction,
  RustBuffer unit,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(Uint64, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer ffi_cdk_ffi_rustbuffer_alloc(
  int size,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(ForeignBytes, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer ffi_cdk_ffi_rustbuffer_from_bytes(
  ForeignBytes bytes,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Void Function(RustBuffer, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void ffi_cdk_ffi_rustbuffer_free(
  RustBuffer buf,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<RustBuffer Function(RustBuffer, Uint64, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer ffi_cdk_ffi_rustbuffer_reserve(
  RustBuffer buf,
  int additional,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Void Function(
    Pointer<Void>,
    Pointer<NativeFunction<UniffiRustFutureContinuationCallback>>,
    Pointer<Void>,
  )
>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_poll_u8(
  Pointer<Void> handle,
  Pointer<NativeFunction<UniffiRustFutureContinuationCallback>> callback,
  Pointer<Void> callback_data,
);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_cancel_u8(Pointer<Void> handle);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_free_u8(Pointer<Void> handle);

@Native<Uint8 Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external int ffi_cdk_ffi_rust_future_complete_u8(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Void Function(
    Pointer<Void>,
    Pointer<NativeFunction<UniffiRustFutureContinuationCallback>>,
    Pointer<Void>,
  )
>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_poll_i8(
  Pointer<Void> handle,
  Pointer<NativeFunction<UniffiRustFutureContinuationCallback>> callback,
  Pointer<Void> callback_data,
);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_cancel_i8(Pointer<Void> handle);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_free_i8(Pointer<Void> handle);

@Native<Int8 Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external int ffi_cdk_ffi_rust_future_complete_i8(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Void Function(
    Pointer<Void>,
    Pointer<NativeFunction<UniffiRustFutureContinuationCallback>>,
    Pointer<Void>,
  )
>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_poll_u16(
  Pointer<Void> handle,
  Pointer<NativeFunction<UniffiRustFutureContinuationCallback>> callback,
  Pointer<Void> callback_data,
);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_cancel_u16(Pointer<Void> handle);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_free_u16(Pointer<Void> handle);

@Native<Uint16 Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external int ffi_cdk_ffi_rust_future_complete_u16(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Void Function(
    Pointer<Void>,
    Pointer<NativeFunction<UniffiRustFutureContinuationCallback>>,
    Pointer<Void>,
  )
>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_poll_i16(
  Pointer<Void> handle,
  Pointer<NativeFunction<UniffiRustFutureContinuationCallback>> callback,
  Pointer<Void> callback_data,
);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_cancel_i16(Pointer<Void> handle);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_free_i16(Pointer<Void> handle);

@Native<Int16 Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external int ffi_cdk_ffi_rust_future_complete_i16(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Void Function(
    Pointer<Void>,
    Pointer<NativeFunction<UniffiRustFutureContinuationCallback>>,
    Pointer<Void>,
  )
>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_poll_u32(
  Pointer<Void> handle,
  Pointer<NativeFunction<UniffiRustFutureContinuationCallback>> callback,
  Pointer<Void> callback_data,
);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_cancel_u32(Pointer<Void> handle);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_free_u32(Pointer<Void> handle);

@Native<Uint32 Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external int ffi_cdk_ffi_rust_future_complete_u32(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Void Function(
    Pointer<Void>,
    Pointer<NativeFunction<UniffiRustFutureContinuationCallback>>,
    Pointer<Void>,
  )
>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_poll_i32(
  Pointer<Void> handle,
  Pointer<NativeFunction<UniffiRustFutureContinuationCallback>> callback,
  Pointer<Void> callback_data,
);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_cancel_i32(Pointer<Void> handle);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_free_i32(Pointer<Void> handle);

@Native<Int32 Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external int ffi_cdk_ffi_rust_future_complete_i32(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Void Function(
    Pointer<Void>,
    Pointer<NativeFunction<UniffiRustFutureContinuationCallback>>,
    Pointer<Void>,
  )
>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_poll_u64(
  Pointer<Void> handle,
  Pointer<NativeFunction<UniffiRustFutureContinuationCallback>> callback,
  Pointer<Void> callback_data,
);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_cancel_u64(Pointer<Void> handle);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_free_u64(Pointer<Void> handle);

@Native<Uint64 Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external int ffi_cdk_ffi_rust_future_complete_u64(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Void Function(
    Pointer<Void>,
    Pointer<NativeFunction<UniffiRustFutureContinuationCallback>>,
    Pointer<Void>,
  )
>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_poll_i64(
  Pointer<Void> handle,
  Pointer<NativeFunction<UniffiRustFutureContinuationCallback>> callback,
  Pointer<Void> callback_data,
);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_cancel_i64(Pointer<Void> handle);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_free_i64(Pointer<Void> handle);

@Native<Int64 Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external int ffi_cdk_ffi_rust_future_complete_i64(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Void Function(
    Pointer<Void>,
    Pointer<NativeFunction<UniffiRustFutureContinuationCallback>>,
    Pointer<Void>,
  )
>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_poll_f32(
  Pointer<Void> handle,
  Pointer<NativeFunction<UniffiRustFutureContinuationCallback>> callback,
  Pointer<Void> callback_data,
);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_cancel_f32(Pointer<Void> handle);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_free_f32(Pointer<Void> handle);

@Native<Float Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external double ffi_cdk_ffi_rust_future_complete_f32(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Void Function(
    Pointer<Void>,
    Pointer<NativeFunction<UniffiRustFutureContinuationCallback>>,
    Pointer<Void>,
  )
>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_poll_f64(
  Pointer<Void> handle,
  Pointer<NativeFunction<UniffiRustFutureContinuationCallback>> callback,
  Pointer<Void> callback_data,
);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_cancel_f64(Pointer<Void> handle);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_free_f64(Pointer<Void> handle);

@Native<Double Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external double ffi_cdk_ffi_rust_future_complete_f64(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Void Function(
    Pointer<Void>,
    Pointer<NativeFunction<UniffiRustFutureContinuationCallback>>,
    Pointer<Void>,
  )
>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_poll_rust_buffer(
  Pointer<Void> handle,
  Pointer<NativeFunction<UniffiRustFutureContinuationCallback>> callback,
  Pointer<Void> callback_data,
);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_cancel_rust_buffer(Pointer<Void> handle);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_free_rust_buffer(Pointer<Void> handle);

@Native<RustBuffer Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external RustBuffer ffi_cdk_ffi_rust_future_complete_rust_buffer(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<
  Void Function(
    Pointer<Void>,
    Pointer<NativeFunction<UniffiRustFutureContinuationCallback>>,
    Pointer<Void>,
  )
>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_poll_void(
  Pointer<Void> handle,
  Pointer<NativeFunction<UniffiRustFutureContinuationCallback>> callback,
  Pointer<Void> callback_data,
);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_cancel_void(Pointer<Void> handle);

@Native<Void Function(Pointer<Void>)>(assetId: _uniffiAssetId)
external void ffi_cdk_ffi_rust_future_free_void(Pointer<Void> handle);

@Native<Void Function(Pointer<Void>, Pointer<RustCallStatus>)>(
  assetId: _uniffiAssetId,
)
external void ffi_cdk_ffi_rust_future_complete_void(
  Pointer<Void> handle,
  Pointer<RustCallStatus> uniffiStatus,
);

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_create_wallet_db();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_auth_proof();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_conditions();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_contact_info();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_create_request_params();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_invoice();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_key_set();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_key_set_info();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_keys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_mint_info();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_mint_version();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_nuts();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_payment_request();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_proof_info();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_proof_state_update();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_receive_options();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_send_memo();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_send_options();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_subscribe_params();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_decode_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_auth_proof();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_conditions();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_contact_info();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_create_request_params();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_key_set();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_key_set_info();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_keys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_mint_info();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_mint_version();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_nuts();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_proof_info();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_proof_state_update();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_receive_options();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_send_memo();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_send_options();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_subscribe_params();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_encode_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_generate_mnemonic();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_init_default_logging();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_init_logging();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_mint_quote_amount_mintable();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_mint_quote_is_expired();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_mint_quote_total_amount();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_mnemonic_to_entropy();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_func_npubcash_derive_secret_key_from_seed();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_npubcash_get_pubkey();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_npubcash_quote_to_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_proof_has_dleq();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_proof_is_active();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_proof_sign_p2pk();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_proof_verify_dleq();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_proof_verify_htlc();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_proof_y();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_proofs_total_amount();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_func_transaction_matches_conditions();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_activesubscription_id();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_activesubscription_recv();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_activesubscription_try_recv();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_nostrwaitinfo_pubkey();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_nostrwaitinfo_relays();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_npubcashclient_get_quotes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_npubcashclient_set_mint_url();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_paymentrequest_amount();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_paymentrequest_description();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_paymentrequest_mints();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_paymentrequest_payment_id();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_paymentrequest_single_use();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_paymentrequest_to_string_encoded();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_paymentrequest_transports();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_paymentrequest_unit();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_paymentrequestpayload_id();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_paymentrequestpayload_memo();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_paymentrequestpayload_mint();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_paymentrequestpayload_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_paymentrequestpayload_unit();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedmelt_amount();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedmelt_cancel();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_preparedmelt_change_amount_without_swap();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedmelt_confirm();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedmelt_confirm_with_options();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedmelt_fee_reserve();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_preparedmelt_fee_savings_without_swap();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedmelt_input_fee();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_preparedmelt_input_fee_without_swap();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedmelt_operation_id();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedmelt_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedmelt_quote_id();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedmelt_requires_swap();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedmelt_swap_fee();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedmelt_total_fee();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedmelt_total_fee_with_swap();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedsend_amount();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedsend_cancel();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedsend_confirm();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedsend_fee();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedsend_operation_id();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_preparedsend_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_token_encode();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_token_htlc_hashes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_token_locktimes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_token_memo();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_token_mint_url();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_token_p2pk_pubkeys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_token_p2pk_refund_pubkeys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_token_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_token_proofs_simple();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_token_spending_conditions();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_token_to_raw_bytes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_token_unit();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_token_value();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_calculate_fee();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_check_all_pending_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_check_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_check_proofs_spent();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_check_send_status();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_fetch_mint_info();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_fetch_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_get_active_keyset();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_get_keyset_fees_by_id();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_get_pending_sends();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_get_proofs_by_states();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_get_proofs_for_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_get_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_get_unspent_auth_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_list_transactions();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_load_mint_info();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_melt_bip353_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_melt_human_readable();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_wallet_melt_lightning_address_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_mint();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_mint_blind_auth();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_mint_unified();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_mint_url();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_pay_request();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_prepare_melt();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_prepare_melt_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_prepare_send();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_receive();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_receive_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_refresh_access_token();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_refresh_keysets();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_restore();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_revert_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_revoke_send();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_set_cat();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_set_metadata_cache_ttl();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_set_refresh_token();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_subscribe();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_swap();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_total_balance();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_total_pending_balance();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_total_reserved_balance();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_unit();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_wallet_verify_token_dleq();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_mint();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_mints();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_mint_keysets();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_keyset_by_id();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_mint_quotes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletdatabase_get_unissued_mint_quotes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_melt_quotes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_keys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_proofs_by_ys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_balance();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_list_transactions();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_kv_read();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_kv_list();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_kv_write();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_kv_remove();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_update_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletdatabase_update_proofs_state();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_add_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_remove_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_update_mint_url();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletdatabase_increment_keyset_counter();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_add_mint();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_remove_mint();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_add_mint_keysets();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_add_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_remove_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_add_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_remove_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_add_keys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_remove_keys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_add_saga();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_get_saga();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_update_saga();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_delete_saga();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletdatabase_get_incomplete_sagas();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_reserve_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_release_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletdatabase_get_reserved_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_reserve_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_release_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_reserve_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletdatabase_release_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_keys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_mint();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_mint_keysets();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_saga();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_delete_saga();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_balance();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_incomplete_sagas();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_keys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_keyset_by_id();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_melt_quotes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_mint();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_mint_keysets();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_mint_quotes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_mints();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_proofs_by_ys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_reserved_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_saga();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_unissued_mint_quotes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_increment_keyset_counter();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_kv_list();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_kv_read();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_kv_remove();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_kv_write();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_list_transactions();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_release_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_release_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_release_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_remove_keys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_remove_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_remove_mint();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_remove_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_remove_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_reserve_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_reserve_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_reserve_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_update_mint_url();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_update_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_update_proofs_state();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_update_saga();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletrepository_create_wallet();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletrepository_get_balances();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletrepository_get_wallet();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletrepository_get_wallets();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletrepository_has_mint();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletrepository_remove_wallet();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletrepository_set_metadata_cache_ttl_for_all_mints();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletrepository_set_metadata_cache_ttl_for_mint();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_keys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_mint();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_mint_keysets();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_saga();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_delete_saga();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_balance();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_incomplete_sagas();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_keys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_keyset_by_id();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_melt_quotes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_mint();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_mint_keysets();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_mint_quotes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_mints();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_proofs_by_ys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_reserved_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_saga();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_unissued_mint_quotes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_increment_keyset_counter();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_kv_list();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_kv_read();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_kv_remove();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_kv_write();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_list_transactions();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_release_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_release_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_release_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_remove_keys();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_remove_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_remove_mint();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_remove_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_remove_transaction();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_reserve_melt_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_reserve_mint_quote();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_reserve_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_update_mint_url();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_update_proofs();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_update_proofs_state();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_update_saga();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_constructor_npubcashclient_new();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_constructor_paymentrequest_from_string();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_constructor_paymentrequestpayload_from_string();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_constructor_token_decode();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_constructor_token_from_raw_bytes();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_constructor_token_from_string();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_constructor_wallet_new();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_constructor_walletpostgresdatabase_new();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_constructor_walletrepository_new();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_constructor_walletrepository_new_with_proxy();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int uniffi_cdk_ffi_checksum_constructor_walletsqlitedatabase_new();

@Native<Uint16 Function()>(assetId: _uniffiAssetId)
external int
uniffi_cdk_ffi_checksum_constructor_walletsqlitedatabase_new_in_memory();

@Native<Uint32 Function()>(assetId: _uniffiAssetId)
external int ffi_cdk_ffi_uniffi_contract_version();

void _checkApiVersion() {
  final bindingsVersion = 30;
  final scaffoldingVersion = ffi_cdk_ffi_uniffi_contract_version();
  if (bindingsVersion != scaffoldingVersion) {
    throw UniffiInternalError.panicked(
      "UniFFI contract version mismatch: bindings version \$bindingsVersion, scaffolding version \$scaffoldingVersion",
    );
  }
}

void _checkApiChecksums() {
  if (uniffi_cdk_ffi_checksum_func_create_wallet_db() != 38981) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_auth_proof() != 22357) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_conditions() != 18453) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_contact_info() != 40231) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_create_request_params() != 8102) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_invoice() != 20311) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_key_set() != 64139) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_key_set_info() != 26774) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_keys() != 38114) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_melt_quote() != 31843) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_mint_info() != 4255) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_mint_quote() != 12595) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_mint_version() != 54734) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_nuts() != 23702) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_payment_request() != 36715) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_proof_info() != 19899) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_proof_state_update() != 25192) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_receive_options() != 46457) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_send_memo() != 6016) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_send_options() != 43827) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_subscribe_params() != 6793) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_decode_transaction() != 48687) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_auth_proof() != 15755) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_conditions() != 48516) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_contact_info() != 44629) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_create_request_params() != 21001) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_key_set() != 10879) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_key_set_info() != 18895) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_keys() != 20045) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_melt_quote() != 25080) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_mint_info() != 31825) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_mint_quote() != 52375) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_mint_version() != 3369) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_nuts() != 30942) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_proof_info() != 32664) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_proof_state_update() != 62126) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_receive_options() != 34534) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_send_memo() != 10559) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_send_options() != 12512) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_subscribe_params() != 58897) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_encode_transaction() != 38295) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_generate_mnemonic() != 17512) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_init_default_logging() != 4192) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_init_logging() != 13465) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_mint_quote_amount_mintable() != 6913) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_mint_quote_is_expired() != 6685) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_mint_quote_total_amount() != 34269) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_mnemonic_to_entropy() != 58572) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_npubcash_derive_secret_key_from_seed() !=
      22494) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_npubcash_get_pubkey() != 28438) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_npubcash_quote_to_mint_quote() != 58675) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_proof_has_dleq() != 56072) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_proof_is_active() != 26064) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_proof_sign_p2pk() != 61649) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_proof_verify_dleq() != 1267) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_proof_verify_htlc() != 24106) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_proof_y() != 55958) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_proofs_total_amount() != 58202) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_func_transaction_matches_conditions() != 45503) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_activesubscription_id() != 53295) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_activesubscription_recv() != 64493) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_activesubscription_try_recv() != 8454) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_nostrwaitinfo_pubkey() != 8372) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_nostrwaitinfo_relays() != 40910) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_npubcashclient_get_quotes() != 64169) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_npubcashclient_set_mint_url() != 8738) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_paymentrequest_amount() != 17196) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_paymentrequest_description() != 30652) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_paymentrequest_mints() != 17730) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_paymentrequest_payment_id() != 12834) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_paymentrequest_single_use() != 17480) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_paymentrequest_to_string_encoded() !=
      63792) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_paymentrequest_transports() != 60834) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_paymentrequest_unit() != 31184) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_paymentrequestpayload_id() != 27515) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_paymentrequestpayload_memo() != 56685) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_paymentrequestpayload_mint() != 42962) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_paymentrequestpayload_proofs() != 56354) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_paymentrequestpayload_unit() != 9118) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_amount() != 25790) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_cancel() != 14185) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_change_amount_without_swap() !=
      59536) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_confirm() != 44853) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_confirm_with_options() !=
      32808) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_fee_reserve() != 24820) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_fee_savings_without_swap() !=
      13657) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_input_fee() != 22331) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_input_fee_without_swap() !=
      59303) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_operation_id() != 52002) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_proofs() != 22010) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_quote_id() != 54442) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_requires_swap() != 26720) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_swap_fee() != 15287) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_total_fee() != 37542) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedmelt_total_fee_with_swap() !=
      44787) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedsend_amount() != 62180) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedsend_cancel() != 48000) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedsend_confirm() != 5962) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedsend_fee() != 37119) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedsend_operation_id() != 33181) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_preparedsend_proofs() != 87) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_token_encode() != 53245) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_token_htlc_hashes() != 14335) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_token_locktimes() != 44524) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_token_memo() != 28883) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_token_mint_url() != 16820) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_token_p2pk_pubkeys() != 56348) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_token_p2pk_refund_pubkeys() != 16072) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_token_proofs() != 60002) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_token_proofs_simple() != 23555) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_token_spending_conditions() != 55293) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_token_to_raw_bytes() != 25396) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_token_unit() != 55723) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_token_value() != 22223) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_calculate_fee() != 1751) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_check_all_pending_proofs() !=
      7291) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_check_mint_quote() != 30988) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_check_proofs_spent() != 31942) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_check_send_status() != 48245) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_fetch_mint_info() != 41951) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_fetch_mint_quote() != 45745) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_get_active_keyset() != 55608) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_get_keyset_fees_by_id() != 51180) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_get_pending_sends() != 56442) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_get_proofs_by_states() != 49189) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_get_proofs_for_transaction() !=
      4480) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_get_transaction() != 62811) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_get_unspent_auth_proofs() !=
      31137) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_list_transactions() != 20673) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_load_mint_info() != 12995) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_melt_bip353_quote() != 56775) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_melt_human_readable() != 19936) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_melt_lightning_address_quote() !=
      35934) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_melt_quote() != 14346) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_mint() != 9725) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_mint_blind_auth() != 16547) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_mint_quote() != 4487) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_mint_unified() != 4620) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_mint_url() != 6804) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_pay_request() != 63052) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_prepare_melt() != 18573) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_prepare_melt_proofs() != 47387) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_prepare_send() != 18579) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_receive() != 34397) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_receive_proofs() != 40857) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_refresh_access_token() != 63251) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_refresh_keysets() != 60028) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_restore() != 15985) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_revert_transaction() != 31115) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_revoke_send() != 52137) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_set_cat() != 29016) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_set_metadata_cache_ttl() != 24324) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_set_refresh_token() != 28616) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_subscribe() != 26376) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_swap() != 45250) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_total_balance() != 37325) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_total_pending_balance() != 26959) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_total_reserved_balance() != 65325) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_unit() != 33359) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_wallet_verify_token_dleq() != 53589) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_mint() != 55827) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_mints() != 42422) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_mint_keysets() !=
      65074) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_keyset_by_id() !=
      48623) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_mint_quote() != 27503) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_mint_quotes() != 1247) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_unissued_mint_quotes() !=
      14181) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_melt_quote() != 58705) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_melt_quotes() !=
      27131) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_keys() != 15412) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_proofs() != 2478) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_proofs_by_ys() !=
      63784) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_balance() != 34149) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_transaction() !=
      56818) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_list_transactions() !=
      46759) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_kv_read() != 55817) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_kv_list() != 45446) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_kv_write() != 46981) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_kv_remove() != 47987) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_update_proofs() != 18069) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_update_proofs_state() !=
      42820) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_add_transaction() !=
      46129) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_remove_transaction() !=
      1866) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_update_mint_url() !=
      13330) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_increment_keyset_counter() !=
      54754) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_add_mint() != 16923) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_remove_mint() != 4222) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_add_mint_keysets() !=
      36430) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_add_mint_quote() != 27831) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_remove_mint_quote() !=
      55242) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_add_melt_quote() != 31104) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_remove_melt_quote() !=
      12796) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_add_keys() != 39274) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_remove_keys() != 11073) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_add_saga() != 61235) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_saga() != 48865) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_update_saga() != 19170) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_delete_saga() != 41562) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_incomplete_sagas() !=
      26098) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_reserve_proofs() != 49254) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_release_proofs() != 47667) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_get_reserved_proofs() !=
      62407) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_reserve_melt_quote() !=
      52928) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_release_melt_quote() !=
      1540) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_reserve_mint_quote() !=
      48388) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletdatabase_release_mint_quote() !=
      15741) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_keys() !=
      56387) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_melt_quote() !=
      14392) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_mint() !=
      29694) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_mint_keysets() !=
      63125) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_mint_quote() !=
      18330) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_saga() !=
      62408) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_add_transaction() !=
      60425) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_delete_saga() !=
      52539) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_balance() !=
      26475) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_incomplete_sagas() !=
      55228) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_keys() !=
      1364) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_keyset_by_id() !=
      47211) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_melt_quote() !=
      15686) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_melt_quotes() !=
      61301) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_mint() !=
      1440) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_mint_keysets() !=
      52552) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_mint_quote() !=
      62393) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_mint_quotes() !=
      37612) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_mints() !=
      51201) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_proofs() !=
      17876) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_proofs_by_ys() !=
      18842) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_reserved_proofs() !=
      35811) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_saga() !=
      30028) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_transaction() !=
      16334) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_get_unissued_mint_quotes() !=
      431) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_increment_keyset_counter() !=
      11359) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_kv_list() !=
      61533) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_kv_read() != 9724) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_kv_remove() !=
      55077) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_kv_write() !=
      45615) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_list_transactions() !=
      57613) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_release_melt_quote() !=
      33492) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_release_mint_quote() !=
      54182) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_release_proofs() !=
      18557) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_remove_keys() !=
      3270) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_remove_melt_quote() !=
      13050) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_remove_mint() !=
      52702) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_remove_mint_quote() !=
      40583) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_remove_transaction() !=
      19625) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_reserve_melt_quote() !=
      25305) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_reserve_mint_quote() !=
      51050) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_reserve_proofs() !=
      39792) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_update_mint_url() !=
      44171) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_update_proofs() !=
      54294) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_update_proofs_state() !=
      58913) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletpostgresdatabase_update_saga() !=
      21044) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletrepository_create_wallet() !=
      32021) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletrepository_get_balances() != 25632) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletrepository_get_wallet() != 57352) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletrepository_get_wallets() != 2280) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletrepository_has_mint() != 64747) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletrepository_remove_wallet() !=
      57714) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletrepository_set_metadata_cache_ttl_for_all_mints() !=
      27302) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletrepository_set_metadata_cache_ttl_for_mint() !=
      23477) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_keys() != 5879) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_melt_quote() !=
      34892) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_mint() != 44674) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_mint_keysets() !=
      13932) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_mint_quote() !=
      62077) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_saga() != 31549) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_add_transaction() !=
      26193) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_delete_saga() !=
      25611) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_balance() !=
      3300) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_incomplete_sagas() !=
      49190) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_keys() != 41498) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_keyset_by_id() !=
      37425) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_melt_quote() !=
      31302) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_melt_quotes() !=
      1543) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_mint() != 23917) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_mint_keysets() !=
      13541) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_mint_quote() !=
      57388) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_mint_quotes() !=
      50536) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_mints() !=
      14065) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_proofs() !=
      48231) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_proofs_by_ys() !=
      13344) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_reserved_proofs() !=
      55044) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_saga() != 59736) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_transaction() !=
      52949) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_get_unissued_mint_quotes() !=
      21540) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_increment_keyset_counter() !=
      61780) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_kv_list() != 61619) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_kv_read() != 16906) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_kv_remove() !=
      63132) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_kv_write() != 37177) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_list_transactions() !=
      22793) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_release_melt_quote() !=
      7347) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_release_mint_quote() !=
      48218) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_release_proofs() !=
      3426) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_remove_keys() !=
      64071) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_remove_melt_quote() !=
      16969) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_remove_mint() !=
      32740) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_remove_mint_quote() !=
      55358) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_remove_transaction() !=
      38835) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_reserve_melt_quote() !=
      17298) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_reserve_mint_quote() !=
      22470) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_reserve_proofs() !=
      20833) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_update_mint_url() !=
      2109) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_update_proofs() !=
      23133) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_update_proofs_state() !=
      51402) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_method_walletsqlitedatabase_update_saga() !=
      32010) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_constructor_npubcashclient_new() != 49637) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_constructor_paymentrequest_from_string() !=
      4890) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_constructor_paymentrequestpayload_from_string() !=
      31548) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_constructor_token_decode() != 17843) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_constructor_token_from_raw_bytes() != 53011) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_constructor_token_from_string() != 43724) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_constructor_wallet_new() != 18752) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_constructor_walletpostgresdatabase_new() !=
      43914) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_constructor_walletrepository_new() != 16691) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_constructor_walletrepository_new_with_proxy() !=
      34392) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_constructor_walletsqlitedatabase_new() != 10235) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
  if (uniffi_cdk_ffi_checksum_constructor_walletsqlitedatabase_new_in_memory() !=
      41747) {
    throw UniffiInternalError.panicked("UniFFI API checksum mismatch");
  }
}

void ensureInitialized() {
  _checkApiVersion();
  _checkApiChecksums();
}

@Deprecated("Use ensureInitialized instead")
void initialize() {
  ensureInitialized();
}
