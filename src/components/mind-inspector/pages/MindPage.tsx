import React, { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { MindState } from '../../../types';
import ComicOverview, { type ComicCharacter, type ComicOverviewControl } from './ComicOverview';
interface MoodData {
  valence: number;
  arousal: number;
  primary_emotion: string;
  mood_label?: string;
  emotion_label?: string;
  primary_intensity?: number;
  // 扩展字段（后端注入的兼容字段）
  energy?: number;
  focus?: number;
  intimacy?: number;
  trust?: number;
  positive_affect?: number;
  negative_affect?: number;
  mood_score?: number;
  mood_emotion?: string;
  mood_secondary?: string;
  emotion_key?: string;
  fatigue?: number;
  stress?: number;
  relationship_score?: number;
}

/** 心理学全维度快照（get_psychology_state 返回） */
interface PsychologySnapshot {
  emotion: {
    joy: number;
    sadness: number;
    anger: number;
    fear: number;
    closeness: number;
    loneliness: number;
    curiosity: number;
  };
  needs: {
    belonging: number;
    autonomy: number;
    security: number;
    novelty: number;
    expression: number;
  };
  relationship: {
    trust: number;
    intimacy: number;
    respect: number;
    dependency: number;
    familiarity: number;
  };
}


const metrics = (snapshot: PsychologySnapshot | null, group: 'emotion' | 'needs' | 'relationship', labels: Record<string,string>): Array<[string,number|string]> => Object.entries(labels).map(([key,label])=>[label, typeof (snapshot?.[group] as unknown as Record<string,number>)?.[key] === 'number' ? Math.round(Math.max(0,Math.min(1,(snapshot![group] as unknown as Record<string,number>)[key]))*100) : '—']);
const thoughtText = (raw: string): string => { try { const obj=JSON.parse(raw); for(const key of ['content','thinking','thought','text','monologue','reply','answer'])if(typeof obj[key]==='string'&&obj[key].trim())return obj[key].trim(); }catch{/* Plain text. */}return raw.trim(); };
function character(name:string,mind:MindState|null,mood:MoodData|null,psych:PsychologySnapshot|null,loading:boolean,error:string|null,t:TFunction,presence?:{state:string;since:number}):ComicCharacter{
 const elapsed=presence?Math.max(0,Math.floor(Date.now()/1000-presence.since)):0;
 const state=presence?t(`comic.${['online','busy','rest','offline'].includes(presence.state)?presence.state:'unknown'}`):t('comic.unknown');
 const labels:Record<string,string>={joy:'喜悦',happiness:'喜悦',sadness:'悲伤',anger:'愤怒',fear:'恐惧',closeness:'亲近',affection:'亲近',loneliness:'孤独',curiosity:'好奇'};
 const primaryEmotion=labels[mood?.primary_emotion??'']??Object.entries(psych?.emotion??{}).sort((a,b)=>b[1]-a[1]).map(([key])=>labels[key])[0]??'暂无情绪';
 return {status:loading?'loading':'ready',name,primaryEmotion,presence:state+(presence?` · ${t(elapsed<60?'comic.seconds':elapsed<3600?'comic.minutes':'comic.hours',{n:elapsed<60?elapsed:elapsed<3600?Math.floor(elapsed/60):Math.floor(elapsed/3600)})}`:''),thought:loading?t('comic.loadingThought'):error?t('comic.failedThought'):mind?.inner_monologue_enabled===false?t('comic.disabledThought'):thoughtText(mind?.current_thought??'')||t('comic.quietThought'),goal:error?t('comic.failedGoal'):mind?.goals.find(g=>g.active)?.description||mind?.goals[0]?.description||t('comic.quietGoal'),mood:loading?t('comic.loadingMood'):error?t('comic.failedMood'):mood?.mood_label||mood?.emotion_label||mood?.primary_emotion||t('comic.noMood'),valence:error?null:mood?.valence??null,arousal:error?null:mood?.arousal??null,emotions:metrics(error?null:psych,'emotion',{joy:'喜悦',sadness:'悲伤',anger:'愤怒',fear:'恐惧',closeness:'亲近',loneliness:'孤独',curiosity:'好奇'}),needs:metrics(error?null:psych,'needs',{belonging:'归属',autonomy:'自主',security:'安全',novelty:'新奇',expression:'表达'}),relations:metrics(error?null:psych,'relationship',{trust:'信任',intimacy:'亲密',respect:'尊重',dependency:'依赖',familiarity:'熟悉'}),error};
}
const LiveMindView: React.FC<ComicOverviewControl> = ({ pageIndex, onTurn, active, functional }) => {
  const { t } = useTranslation();
  const [vivianMind, setVivianMind] = useState<MindState | null>(null);
  const [nanaMind, setNanaMind] = useState<MindState | null>(null);
  const [vivianMood, setVivianMood] = useState<MoodData | null>(null);
  const [nanaMood, setNanaMood] = useState<MoodData | null>(null);
  const [vivianPsych, setVivianPsych] = useState<PsychologySnapshot | null>(null);
  const [nanaPsych, setNanaPsych] = useState<PsychologySnapshot | null>(null);
  const [loadingV, setLoadingV] = useState(true);
  const [loadingN, setLoadingN] = useState(true);
  const [errorV, setErrorV] = useState<string | null>(null);
  const [errorN, setErrorN] = useState<string | null>(null);
  // 各角色在场状态（presence）：online / busy / rest / offline，与 ChatWindow 数据源保持一致
  // 同时记录进入当前状态的 Unix 秒时间戳，用于心智页持续时长显示
  const [presenceStates, setPresenceStates] = useState<Record<string, { state: string; since: number }>>({});

  useEffect(() => {
    let cancelled = false;
    const fetchVivian = async () => {
      try {
        const [mind, mood, psych] = await Promise.all([
          invoke<MindState>('get_mind_state', { characterId: 'vivian' }),
          invoke<MoodData>('get_current_mood', { characterId: 'vivian' }),
          invoke<PsychologySnapshot>('get_psychology_state', { characterId: 'vivian' }),
        ]);
        if (!cancelled) {
          setVivianMind(mind);
          setVivianMood(mood);
          setVivianPsych(psych);
          setErrorV(null);
        }
      } catch (e) {
        if (!cancelled) setErrorV(String(e));
      } finally {
        if (!cancelled) setLoadingV(false);
      }
    };
    const fetchNana = async () => {
      try {
        const [mind, mood, psych] = await Promise.all([
          invoke<MindState>('get_mind_state', { characterId: 'nana' }),
          invoke<MoodData>('get_current_mood', { characterId: 'nana' }),
          invoke<PsychologySnapshot>('get_psychology_state', { characterId: 'nana' }),
        ]);
        if (!cancelled) {
          setNanaMind(mind);
          setNanaMood(mood);
          setNanaPsych(psych);
          setErrorN(null);
        }
      } catch (e) {
        if (!cancelled) setErrorN(String(e));
      } finally {
        if (!cancelled) setLoadingN(false);
      }
    };
    void fetchVivian();
    void fetchNana();
    const id = window.setInterval(() => {
      void fetchVivian();
      void fetchNana();
    }, 5_000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  // 拉取在场状态 + 监听 presence:changed 事件，保持与微信面板显示一致
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void (async () => {
      try {
        const states = await invoke<Array<{ character_id: string; state: string; since: number }>>('get_all_presence_states');
        if (cancelled) return;
        const map: Record<string, { state: string; since: number }> = {};
        for (const s of states) map[s.character_id] = { state: s.state, since: s.since ?? Date.now() / 1000 };
        setPresenceStates(map);
      } catch { /* ignore */ }
      try {
        unlisten = await listen<{ character_id: string; to: string }>('presence:changed', (e) => {
          if (!e.payload?.character_id) return;
          setPresenceStates((prev) => ({
            ...prev,
            [e.payload.character_id]: { state: e.payload.to, since: Date.now() / 1000 },
          }));
        });
        // 迟到 resolve 兜底：cleanup 先于监听器注册完成时，当场解绑防泄漏
        if (cancelled) { unlisten?.(); unlisten = null; }
      } catch { /* ignore */ }
    })();
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  return <ComicOverview pageIndex={pageIndex} onTurn={onTurn} active={active} functional={functional} data={{vivian:character('Vivian',vivianMind,vivianMood,vivianPsych,loadingV,errorV,t,presenceStates.vivian),nana:character('Nana',nanaMind,nanaMood,nanaPsych,loadingN,errorN,t,presenceStates.nana)}} />;
};
export default LiveMindView;
