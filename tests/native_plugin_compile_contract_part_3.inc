    {
        // The reassembler itself is Rust-owned and covered by its unit tests;
        // this keeps the shared-memory ring contract exercised from C++.
        auto extended = std::make_unique<Hirari::Core::Plugins::MidiExtendedMessageRing>();
        std::array<uint8_t, 480> payload{};
        for (auto& byte : payload) byte = 0x55;
        payload.back() = 0xaa;
        if (!extended->push(12, 3, payload.data(), payload.size()) ||
            extended->size() != 1) return 57;
        Hirari::Core::Plugins::MidiExtendedMessageRing::Message message;
        if (!extended->pop(message) || message.size != payload.size() ||
            message.sampleOffset != 12 || message.articulationId != 3 ||
            message.data[0] != 0x55 || message.data[message.size - 1] != 0xaa) return 58;
    }
    return 0;
}
