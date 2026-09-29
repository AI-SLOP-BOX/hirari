#pragma once
#include "undo_manager.hpp"
#include "../midi_region.hpp"

namespace Hirari::Core::Undo {

/**
 * @class MoveNoteCommand
 * @brief Logic Pro Style single/multi-note movement command with undo.
 */
class MoveNoteCommand : public Command {
public:
    MoveNoteCommand(MIDINote& note, double newTime, int newPitch)
        : m_note(note), m_oldTime(note.startBeat), m_oldPitch(note.pitch),
          m_newTime(newTime), m_newPitch(newPitch) {}
    
    void execute() override { m_note.startBeat = m_newTime; m_note.pitch = m_newPitch; }
    void undo() override { m_note.startBeat = m_oldTime; m_note.pitch = m_oldPitch; }
    std::string getName() const override { return "Move Note"; }

private:
    MIDINote& m_note;
    double m_oldTime, m_newTime;
    int m_oldPitch, m_newPitch;
};

/**
 * @class ChangeVelocityCommand
 * @brief Batch velocity editing with differential undo.
 */
class ChangeVelocityCommand : public Command {
public:
    ChangeVelocityCommand(MIDINote& note, uint8_t newVel)
        : m_note(note), m_oldVel(note.velocity), m_newVel(newVel) {}

    void execute() override { m_note.velocity = m_newVel; }
    void undo() override { m_note.velocity = m_oldVel; }
    std::string getName() const override { return "Change Velocity"; }

private:
    MIDINote& m_note;
    uint8_t m_oldVel, m_newVel;
};

} // namespace Hirari::Core::Undo
