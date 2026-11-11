// Built-in whiteboard templates, drawn with Excalidraw's element skeletons (turned into real
// elements with convertToExcalidrawElements when a board is made from one).

type Skeleton = Record<string, unknown>;
export type Template = { id: string; name: string; description: string; build: () => Skeleton[] };

const box = (x: number, y: number, w: number, h: number, text: string, bg = 'transparent', fontSize = 20): Skeleton => ({
	type: 'rectangle', x, y, width: w, height: h, backgroundColor: bg, fillStyle: 'solid', roundness: { type: 3 },
	label: { text, fontSize, verticalAlign: 'top' }
});
const title = (x: number, y: number, text: string, fontSize = 32): Skeleton => ({ type: 'text', x, y, text, fontSize });
const note = (x: number, y: number, text: string, bg = '#fff3bf'): Skeleton => ({
	type: 'rectangle', x, y, width: 180, height: 110, backgroundColor: bg, fillStyle: 'solid', strokeColor: 'transparent', label: { text, fontSize: 18 }
});

const columns = (heading: string, names: string[], colours: string[], sample: string[]): Skeleton[] => [
	title(0, -80, heading),
	...names.flatMap((n, i) => [box(i * 260, 0, 240, 620, n, colours[i % colours.length]), ...(sample[i] ? [note(i * 260 + 30, 70, sample[i])] : [])])
];

export const TEMPLATES: Template[] = [
	{ id: 'kanban', name: 'Kanban', description: 'To do, doing, done', build: () => columns('Kanban', ['To do', 'Doing', 'Done'], ['#e7f5ff', '#fff9db', '#ebfbee'], ['A first card', '', '']) },
	{ id: 'retro', name: 'Retrospective', description: 'What went well, what to improve, actions', build: () => columns('Retrospective', ['Went well', 'To improve', 'Actions'], ['#ebfbee', '#fff4e6', '#e7f5ff'], ['', '', '']) },
	{
		id: 'swot', name: 'SWOT analysis', description: 'Strengths, weaknesses, opportunities, threats',
		build: () => [title(0, -80, 'SWOT analysis'), box(0, 0, 400, 300, 'Strengths', '#ebfbee'), box(420, 0, 400, 300, 'Weaknesses', '#fff4e6'), box(0, 320, 400, 300, 'Opportunities', '#e7f5ff'), box(420, 320, 400, 300, 'Threats', '#fff5f5')]
	},
	{
		id: 'mindmap', name: 'Mind map', description: 'A central idea with branches',
		build: () => {
			const centre: Skeleton = { type: 'ellipse', id: 'centre', x: 300, y: 250, width: 220, height: 110, backgroundColor: '#d0ebff', fillStyle: 'solid', label: { text: 'Main idea', fontSize: 24 } };
			const spots = [[0, 0], [620, 0], [0, 500], [620, 500]];
			const branches = spots.map(([x, y], i): Skeleton => ({ type: 'rectangle', id: `b${i}`, x, y, width: 200, height: 90, roundness: { type: 3 }, label: { text: `Branch ${i + 1}` } }));
			const arrows = spots.map((_, i): Skeleton => ({ type: 'arrow', x: 410, y: 305, start: { id: 'centre' }, end: { id: `b${i}` } }));
			return [centre, ...branches, ...arrows];
		}
	},
	{
		id: 'week', name: 'Weekly plan', description: 'A column for each working day',
		build: () => columns('Week plan', ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday'], ['#f8f9fa'], [])
	}
];
