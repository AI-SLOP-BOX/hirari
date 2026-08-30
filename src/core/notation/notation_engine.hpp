#pragma once
#include <vector>
#include <string>
#include <map>
#include <memory>
#include <cmath>
#include <algorithm>
#include "../../core/midi_region.hpp"
#include "../../core/engine/track.hpp"
#include "../../scae/notation_intelligence.hpp"

namespace Aura::Core::Notation {

/**
 * @class NotationEngine
 * @brief 【OSS標準への垂直統合：作曲家のための究極の譜面エンジン】
 * 
 * MIDIデータの打ち込みをそのまま「出版クオリティの楽譜」に変換します。
 * MuseScore 4 レベルの記譜法に基づき、アーティキュレーション、強弱、
 * タイ、連符などをAIによって自動的に判断し、譜面に反映します。
 */
class NotationEngine {
public:
    struct Glyph {
        enum { NOTE, CLEF, BARSIDE, TEXT, DYNAMIC } type;
        float x, y;
        std::string symbol;
        int pitch;
        int duration; // 1 = 全音符, 4 = 四分音符, 16 = 十六分音符...
        uint8_t sourceKind = 0;
        uint32_t sourceRegionId = 0;
        uint32_t sourceNoteIndex = 0;
    };

    static NotationEngine& getInstance() { static NotationEngine i; return i; }

    /**
     * @brief RENDER: MIDIリージョンから楽譜のグリフ（描画データ）を生成
     * 1. MIDIノートのクオンタイズ（表示用）
     * 2. キー、拍子記号の判断
     * 3. 五線譜上への最適な配置演算
     */
    std::vector<Glyph> renderTrackToScore(const Engine::Track& track) {
        m_activeTrack = &track;
        auto manifest = SCAE::Intelligence::NotationIntelligence::generateScoreManifest(track);
        std::vector<Glyph> result;
        
        for (const auto& m : manifest) {
            Glyph g;
            g.type = Glyph::NOTE;
            g.x = m.beat * 50.0f; // Scale to view pixels
            g.y = m.staffOffset;
            g.symbol = m.symbolType;
            g.duration = m.duration;
            g.pitch = static_cast<int>(std::lround(m.staffOffset));
            g.sourceKind = m.sourceKind;
            g.sourceRegionId = m.sourceRegionId;
            g.sourceNoteIndex = m.sourceNoteIndex;
            result.push_back(g);
        }
        m_lastGlyphs = result;
        return result;
    }

    std::vector<Glyph> renderMidiToScore(
        const std::vector<std::shared_ptr<MidiRegion>>& regions) {
        // This overload is a display/export view without an owning Track
        // context.  Never let a later edit accidentally target the Track
        // rendered by a previous call.
        m_activeTrack = nullptr;
        const auto manifest = SCAE::Intelligence::NotationIntelligence::generateMidiManifest(regions);
        std::vector<Glyph> result;
        result.reserve(manifest.size());
        for (const auto& m : manifest) {
            Glyph g;
            g.type = Glyph::NOTE;
            g.x = m.beat * 50.0f;
            g.y = m.staffOffset;
            g.symbol = m.symbolType;
            g.duration = m.duration;
            g.pitch = static_cast<int>(std::lround(m.staffOffset));
            g.sourceKind = m.sourceKind;
            g.sourceRegionId = m.sourceRegionId;
            g.sourceNoteIndex = m.sourceNoteIndex;
            result.push_back(g);
        }
        m_lastGlyphs = result;
        return result;
    }

    /**
     * @brief EDIT: 譜面上のグリフを動かすことでMIDIデータを更新（双方向）
     */
    void updateMidiFromNotation(int glyphId, float newX, float newY) {
        (void)tryUpdateGlyph(glyphId, newX, newY);
    }

    bool tryUpdateGlyph(int glyphId, float newX, float newY) {
        if (glyphId < 0 || !std::isfinite(newX) || !std::isfinite(newY)) return false;
        const size_t index = static_cast<size_t>(glyphId);
        if (index >= m_lastGlyphs.size()) return false;
        Glyph& glyph = m_lastGlyphs[index];
        glyph.x = std::max(0.0f, newX);
        glyph.y = newY;
        glyph.pitch = static_cast<int>(std::lround(newY));
        if (glyph.sourceKind == 1) {
            // The notation model stores absolute timeline beat in x and MIDI
            // pitch in y.  Apply the edit to the owning note as well.
            if (!m_activeTrack) return false;
            for (const auto& region : m_activeTrack->getMidiRegions()) {
                if (!region || region->getId() != glyph.sourceRegionId) continue;
                std::vector<MIDINote> notes;
                region->copyProcessedNotes(notes);
                if (glyph.sourceNoteIndex >= notes.size()) return false;
                auto note = notes[glyph.sourceNoteIndex];
                const double absoluteBeat = static_cast<double>(glyph.x) / 50.0;
                const double newStart = absoluteBeat - region->getStartBeat();
                if (!std::isfinite(newStart) || newStart < 0.0) return false;
                note.startBeat = newStart;
                note.pitch = static_cast<uint8_t>(std::clamp(glyph.pitch, 0, 127));
                if (!region->updateNote(glyph.sourceNoteIndex, note)) return false;
                ++m_editGeneration;
                return true;
            }
            return false;
        }
        ++m_editGeneration;
        return true;
    }

    const std::vector<Glyph>& lastGlyphs() const noexcept {
        return m_lastGlyphs;
    }

    uint64_t editGeneration() const noexcept { return m_editGeneration; }
    bool copyGlyphs(std::vector<Glyph>& destination) const {
        destination = m_lastGlyphs;
        return !destination.empty();
    }

private:
    float m_zoom = 1.0f;
    bool m_showLyrics = true;
    std::vector<Glyph> m_lastGlyphs;
    const Engine::Track* m_activeTrack = nullptr;
    uint64_t m_editGeneration = 0;
};

} // namespace Aura::Core::Notation
