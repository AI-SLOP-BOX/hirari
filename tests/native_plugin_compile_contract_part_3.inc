    {
        using Transport = Hirari::Core::Plugins::MidiFragmentReassembler;
        Transport transport;
        Transport::Fragment first{};
        first.messageId = 77; first.index = 0; first.total = 2; first.sampleOffset = 12;
        first.size = 2; first.payload[0] = 0xf0; first.payload[1] = 0x01;
        Transport::Fragment second{};
        second.messageId = 77; second.index = 1; second.total = 2;
        second.size = 2; second.payload[0] = 0x02; second.payload[1] = 0xf7;
        if (transport.push(first) != Transport::Result::Accepted ||
            transport.push(second) != Transport::Result::Complete ||
            !transport.complete() || transport.size() != 4 ||
            transport.data()[0] != 0xf0 || transport.data()[3] != 0xf7) return 54;
        Hirari::Core::MidiBuffer mailbox;
        Transport mailboxTransport;
        if (mailbox.addFragment(mailboxTransport, first) != Transport::Result::Accepted ||
            mailbox.addFragment(mailboxTransport, second) != Transport::Result::Complete ||
            mailbox.size() != 1 || mailbox.getEvents()[0].size != 4 ||
            mailbox.getEvents()[0].data[3] != 0xf7) return 56;
        Transport::Fragment largeFirst{};
        largeFirst.messageId = 91; largeFirst.index = 0; largeFirst.total = 2;
        largeFirst.size = Transport::kFragmentPayloadBytes;
        Transport::Fragment largeSecond{};
        largeSecond.messageId = 91; largeSecond.index = 1; largeSecond.total = 2;
        largeSecond.size = Transport::kFragmentPayloadBytes;
        std::fill(largeFirst.payload.begin(), largeFirst.payload.end(), 0x55);
        std::fill(largeSecond.payload.begin(), largeSecond.payload.end(), 0xaa);
        Transport largeTransport;
        auto extended = std::make_unique<Hirari::Core::Plugins::MidiExtendedMessageRing>();
        if (mailbox.addFragment(largeTransport, largeFirst, extended.get()) != Transport::Result::Accepted ||
            mailbox.addFragment(largeTransport, largeSecond, extended.get()) != Transport::Result::Complete ||
            mailbox.size() != 1 || extended->size() != 1) return 57;
        Hirari::Core::Plugins::MidiExtendedMessageRing::Message largeMessage;
        if (!extended->pop(largeMessage) || largeMessage.size != 2 * Transport::kFragmentPayloadBytes ||
            largeMessage.data[0] != 0x55 || largeMessage.data[largeMessage.size - 1] != 0xaa) return 58;
        Transport::Fragment wrongOrder = first;
        wrongOrder.messageId = 88;
        if (transport.push(wrongOrder) != Transport::Result::Accepted) return 55;
    }
    return 0;
}
