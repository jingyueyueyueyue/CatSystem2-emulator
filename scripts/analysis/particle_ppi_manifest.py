"""Refresh all 131 PPI identities from fixed static evidence and active sources.

No game/VM/plugin is executed. This report describes implemented source routes,
not compiled or verified gameplay. Run from any current directory.
"""
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ARCHIVE = ROOT / 'archive/2026-11'
SOURCE = ROOT / 'simulator/crates/desktop/src'


def main():
    inventory = json.loads((ARCHIVE/'20261117_D_ptclpi_inventory.json').read_text(encoding='utf8'))
    classification = {r['sha256']:r for r in json.loads((ARCHIVE/'20261117_D_ppi_identity_classification.json').read_text(encoding='utf8'))}
    cfg = {r['sha256']:r for r in json.loads((ARCHIVE/'20261117_D_ppi_cfg_clusters.json').read_text(encoding='utf8'))['rows']}
    usage_path = ARCHIVE/'20261117_D_ppi_usage.json'
    usage = {r['sha256']:r for r in json.loads(usage_path.read_text(encoding='utf8'))['rows']} if usage_path.exists() else {}
    models = {}
    for name,model in [('particle_plugin.rs','sdk2.rain'),('particle_rain_field.rs','sdk2.atlas_or_scaled_rain'),('particle_burst.rs','sdk2.bang2'),('particle_common.rs','sdk2.common_plist')]:
        source=(SOURCE/name).read_text(encoding='utf8')
        pattern=r'const RAIN_\w+: &str = "([a-f0-9]{64})"' if name=='particle_plugin.rs' else r'"([a-f0-9]{64})"'
        for sha in re.findall(pattern,source):
            models[sha] = (model,name)
    described = (SOURCE/'particle_family_profiles.rs').read_text(encoding='utf8')
    for sha,kind,profile in re.findall(r'Profile\s*\{\s*sha256:\s*"([a-f0-9]{64})",\s*model:\s*Model::(\w+)(?:\(([\w:]+))?',described):
        models[sha] = (f'sdk2.family.{kind}'+(f'.{profile}' if profile else ''),'particle_family.rs')
    rows = []
    for sha,item in inventory['groups'].items():
        identity = classification[sha]
        active = sha in models
        sdk1 = identity['version'] == 0x10000
        model,consumer = models.get(sha,(None,None))
        evidence=['archive/2026-11/20261117_D_ppi_cfg_clusters.json']
        if sdk1:
            evidence.append('archive/2026-11/20261117_D_ptclpi_host_loader.txt:14e7a6..14e7c9 (type1; versions20000/20001/20002)')
        elif active and model=='sdk2.common_plist':
            evidence.extend(['archive/2026-11/20261117_D_ppi_config_'+sha[:10]+'.txt',
                'archive/2026-11/20261117_D_ppi_common_compare.txt'])
        elif active and model.endswith(('.MESSAGE','.MESSAGE_RANDOM')):
            evidence.extend(['archive/2026-11/20261117_D_ppi_remaining_sprite_'+sha[:10]+'.txt',
                'archive/2026-11/20261117_D_ppi_message_tables.json'])
        elif active and model.endswith(('.TEXT_WORDS','.STREAM_ARROW','.CUBE','.LIGHT','.RIBBON','.WIND_TRAIL','.RADIAL_FLARE','.DAMAGE','.EARLY_SQUARE','.CIRCLE_LIGHT','.CIRCLE_LIGHT_TUNED','.HAPPY','.PHOTON','.DARK','.WC_FALL','.HEART','.INVISI','.BUBBLE','.NUMBER','.ROTATING_BACKGROUND','.BLUR_STRIPS')):
            evidence.append('archive/2026-11/20261117_D_ppi_remaining_sprite_'+sha[:10]+'.txt')
            if model.endswith('.TEXT_WORDS'):
                evidence.append('archive/2026-11/20261117_D_ppi_text_config.json')
            if model.endswith('.CUBE'):
                evidence.extend(['archive/2026-11/20261117_D_ppi_cube_data.json','archive/2026-11/20261117_D_ppi_priority2_callbacks.txt'])
            if model.endswith('.LIGHT'):
                evidence.append('archive/2026-11/20261117_D_ppi_light_atlas.json')
            if model.endswith('.RADIAL_FLARE'):
                evidence.append('archive/2026-11/20261117_D_ppi_radial_atlas.json')
            if model.endswith('.HEART'):
                evidence.append('archive/2026-11/20261117_D_ppi_heart_tables.json')
            if model.endswith(('.BUBBLE','.NUMBER')):
                evidence.append('archive/2026-11/20261117_D_ppi_atlas_emitter_tables.json')
        elif active and model.endswith(('.THUNDER','.FALL_SPARK')):
            evidence.extend(['archive/2026-11/20261117_D_ppi_priority2_'+sha[:10]+'.txt',
                'archive/2026-11/20261117_D_ppi_thunder_data.txt'])
        elif active and model.endswith(('.DUST','.TWINKLE','.FLOATING_SPARK','.A_SPARK')):
            evidence.extend(['archive/2026-11/20261117_D_ppi_priority2_'+sha[:10]+'.txt',
                'archive/2026-11/20261117_D_ppi_flicker_callback.txt'])
        elif active and model.endswith(('.FIRE_PATTERN','.POND','.VORTEX','.BASELINE_SPARK','.TORCH','.SMOKE')):
            evidence.append('archive/2026-11/20261117_D_ppi_geometry_'+sha[:10]+'.txt')
            if model.endswith('.SMOKE'):
                evidence.append('archive/2026-11/20261117_D_ppi_smoke_data.txt')
        elif active and model.endswith(('.SWORD_SPARK','.CREATE_SQUARE','.CONCENT_O','.ROLLING','.ICE','.ICE2')):
            evidence.append('archive/2026-11/20261117_D_ppi_matrix_'+sha[:10]+'.txt')
            if model.endswith(('.ICE','.ICE2')):
                evidence.append('archive/2026-11/20261117_D_ppi_ice_data.txt')
        elif active and model.endswith(('.PROTECT','.PROTECT2')):
            evidence.extend(['archive/2026-11/20261117_D_ppi_geometry_'+sha[:10]+'.txt',
                'archive/2026-11/20261117_D_ppi_protect_tables.json',
                'archive/2026-11/20261117_D_ppi_thunder_data.txt'])
        elif active and model.endswith(('.RADIATE','.ARROW')):
            evidence.append('archive/2026-11/20261117_D_ppi_priority2_'+sha[:10]+'.txt')
        elif active and model.endswith(('.SEED','.SEED_ROT')):
            evidence.extend(['archive/2026-11/20261117_D_ppi_priority2_'+sha[:10]+'.txt',
                'archive/2026-11/20261117_D_ppi_seed_tables.json'])
        elif active and '.Path.' in model:
            if model.endswith(('.TILES','.MIRROR','.EXPRODE','.FIREBASE','.FIREBASE2','.FIREBASE3','.FIREBASE4','.SPARK','.SPARK_EX','.RUBBLE','.ROUND_Y','.ROUND_Z','.ERASE','.ERASE_ATLAS','.STAR','.STAR_LEGACY','.PRISM_PATH','.PRISM_ORBIT','.PRISM_TRAIL')):
                evidence.extend(['archive/2026-11/20261117_D_ppi_priority2_'+sha[:10]+'.txt',
                    'archive/2026-11/20261117_D_ppi_priority2_callbacks.txt',
                    'archive/2026-11/20261116_D_particle669_object_disassembly.txt:14e30f/14e58a'])
            elif model.endswith(('.FIREBALL','.FIREBALL2','.SLASH','.HADOU','.HOTARU_MOVE','.HOTARU_STAY','.WINDBALL','.SPLASH')):
                evidence.append('archive/2026-11/20261117_D_ppi_fire_'+sha[:10]+'.txt')
            elif model.endswith(('.VOLT','.VOLT2','.VOLT3','.VOLT4')):
                evidence.append('archive/2026-11/20261117_D_ppi_volt_'+sha[:10]+'.txt')
            elif model.endswith(('.SIGH','.SHY','.KIRAKIRA','.OKO','.WARUKIRA','.CONCENT','.HOTARU','.ENERGY','.STEAM','.WC_STEAM','.DASH','.RISING3','.RISING4','.SAKURA','.PAPER')):
                evidence.append('archive/2026-11/20261117_D_ppi_followup_'+sha[:10]+'.txt')
            else:
                evidence.append('archive/2026-11/20261117_D_ppi_path_evidence.json')
        elif active and '.Sprite.' in model:
            if model.endswith('::Angry'):
                evidence.extend(['archive/2026-11/20261117_D_ppi_angry_evidence.json','archive/2026-11/20261117_D_ppi_math_callbacks.txt'])
            else:
                evidence.append('archive/2026-11/20261117_D_ppi_'+('priority_' if sha.startswith(('fa95fc','1f6dd7','0a2e3b')) else 'followup_')+sha[:10]+'.txt')
        routes = None
        if active:
            routes = {
                'identity_gate':'simulator/crates/desktop/src/particle_plugin.rs::ParticlePlugin::load',
                'model_source':'simulator/crates/desktop/src/'+consumer,
                'initialize':'ParticlePlugin::start'+(' -> Family::start' if consumer=='particle_family.rs' else ''),
                'update':'LoggingHost::advance_particle_task -> ParticlePlugin::update'+(' -> Family::update' if consumer=='particle_family.rs' else ''),
                'snapshot':'ParticlePlugin::snapshot'+(' -> Family::snapshot' if consumer=='particle_family.rs' else ''),
                'render':'HeadlessPlayer::render_scene_nodes_range -> particle_render::draw_range_with_projection / particle_external::submit',
                'stop':'ParticlePlugin::stop removes active snapshot drawing',
                'release':'LoggingHost::release_particle_program drops plugin, texture and snapshot; reload replaces the owned instance',
            }
            if '.Path.' in model:
                routes['model_source']='simulator/crates/desktop/src/particle_family_path.rs::PathFamily'
                routes['initialize']+=' -> PathFamily::start'
                routes['update']+=' -> PathFamily::update'
                routes['snapshot']+=' -> PathFamily::snapshot'
            if model.endswith('.TEXT_WORDS'):
                routes['initialize']+=' -> PathFamily::load_text_config -> ResourceIndex::read_mode -> real plist word configuration'
            if '.Sprite.' in model:
                routes['model_source']='simulator/crates/desktop/src/particle_family_sprite.rs::SpriteFamily'
                routes['initialize']+=' -> SpriteFamily::start'
                routes['update']+=' -> SpriteFamily::update'
                routes['snapshot']+=' -> SpriteFamily::snapshot'
            if model=='sdk2.common_plist':
                routes['initialize']+=' -> CommonParticles::start -> ResourceIndex::read_mode -> plist scalar configuration'
                routes['update']+=' -> CommonParticles::update'
                routes['snapshot']+=' -> CommonParticles::snapshot'
        u = usage.get(sha,{})
        rows.append({
            'sha256':sha,'model':'sdk2.host-reject.sdk1' if sdk1 else model,'active_source_consumer':{'reject':'ParticlePlugin::load -> FIXED_HOST_REJECTED_SDK1 (no MakeInstance/start/update/render)'} if sdk1 else routes,
            'equivalence_status':'source model registered by exact SHA; full behavioral/pixel equivalence has not been established' if active else 'not established',
            'model_boundaries':(['wc_torch2720 reads object64/68 angular fields not initialized by public1f60/2050; independent model initializes unspecified storage to zero, consumes the real RNG call, and does not claim allocator-dependent equivalence.'] if model and model.endswith('.TORCH') else
                ['Real ptcl%d%03d.plist resource is required at start; missing/archive-unresolved configuration fails explicitly. Consumed numeric fields must be present; uninitialized original allocator fields are not synthesized. Flat XML plist scalar records are supported; binary/nested plist is diagnosed. Scene acceptance has not been verified.'] if model=='sdk2.common_plist' else ['wc_number start/update do not consume configured size or integrate sampled private velocity. Uninitialized object80/48/4c arithmetic does not affect its reachable glyph/phase consumer; allocator-dependent arithmetic equivalence is not claimed.'] if model and model.endswith('.NUMBER') else []),
            'remaining':not active and not sdk1,
            'algorithm_implemented':active,
            'verified_rejection':{'source':'ParticlePlugin::load -> FIXED_HOST_REJECTED_SDK1','type':1,'version':65536,'reason':'Fixed SDK2 host branches around MakeInstance and destroys the unloaded wrapper for SDK1.'} if sdk1 else None,
            'remaining_reason':None if active or sdk1 else (
                'SDK1 algorithm not implemented; fixed SDK2 host rejects version0x10000 before MakeInstance. Rejection is not algorithm coverage.' if sdk1 else
                'No independent initialized/updated/rendered effect model for this exact identity; explicit unsupported diagnostic remains.'),
            'status':'implemented-source-route' if active else ('fixed-sdk2-loader-reject' if sdk1 else 'unimplemented-sdk2'),
            'type':identity['type'],'version':identity['version'],
            'paths':item['paths'],'file_copies':len(item['paths']),
            'method_rvas':[m['entry'] for m in cfg[sha]['methods']],
            'evidence':evidence,
            'priority_evidence':{
                'lexical_kcs_initializer_matches':u.get('initializer_resource_count',0),
                'same_basename_kcs_assets':u.get('named_kcs_resource_count',0),
                'explicit_ppi_string_occurrences':sum(r['explicit_ppi_occurrences'] for r in u.get('initializer_references',[])),
                'static_effect_command_records':u.get('effect_command_count',0),
                'example_effect_commands':u.get('effect_command_references',[])[:2],
                'scope':'Static scene/FES effect-name records and lexical KCS/resource-name correlation, not execution frequency or proof of startup reachability; generic names such as blur/steam can collide.',
            },
        })
    result = {
        'schema':'cs2-particle-ppi-source-manifest/v1',
        'verification_scope':'Static inventory, PE data and source routing only. No compile/type-check/test/gameplay/VM/plugin/PE execution. Implemented source route is not game acceptance.',
        'refresh':'python scripts/analysis/particle_ppi_manifest.py',
        'summary':{
            'identities':len(rows),'files':sum(r['file_copies'] for r in rows),
            'duplicate_file_copies':sum(r['file_copies'] for r in rows)-len(rows),
            'implemented_source_routes':sum(r['status']=='implemented-source-route' for r in rows),
            'accounted_identities':sum(not r['remaining'] for r in rows),
            'remaining':sum(r['remaining'] for r in rows),
            'remaining_sdk2':sum(r['status']=='unimplemented-sdk2' for r in rows),
            'fixed_sdk2_rejected_sdk1':sum(r['status']=='fixed-sdk2-loader-reject' for r in rows),
        },
        'shared_boundaries':[
            'Exact resource SHA authorizes an independent model; names, type/version and CFG similarity alone do not.',
            'Existing Canvas/external renderer, 3D source lookup, texture routing and geometry support keep their own unresolved boundaries.',
            'Aliases produced dynamically by scripts require separate call-argument/dataflow evidence; absence of a lexical match does not make an effect unused.',
        ],
        'rows':sorted(rows,key=lambda r:r['sha256']),
    }
    target=ROOT/'reports/particle_ppi_manifest_20261117.json'
    target.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf8')
    print(json.dumps(result['summary'],indent=2))


if __name__=='__main__':
    main()
